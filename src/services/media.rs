// src/services/media.rs

// dependencies
use crate::storage::StorageBackend;
use anyhow::{Context, anyhow};
use bytes::Bytes;
use image::DynamicImage;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use uuid::Uuid;

/// JPEG quality for derivatives — the "good enough that nobody notices" mark.
const JPEG_QUALITY: u8 = 85;
/// Long-edge caps per variant. `thumb` is a square cover crop; the others
/// preserve aspect ratio and never upscale.
const THUMB_EDGE: u32 = 300;
const MEDIUM_EDGE: u32 = 640;
const LARGE_EDGE: u32 = 1080;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("unsupported or corrupt image")]
    InvalidImage,

    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}

impl actix_web::ResponseError for MediaError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            MediaError::InvalidImage => actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            MediaError::Operation(_) => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// EXIF fields the mockup's meta row displays, stored as JSON on the media
/// row. Served derivatives are re-encoded (EXIF-free), so this DB copy is
/// the only EXIF that exists anywhere — the privacy property falls out of
/// the pipeline instead of being enforced after the fact.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ExifData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_time_original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps_latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps_longitude: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Derivative {
    pub variant: &'static str,
    pub storage_key: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: i64,
}

/// Everything the DB insert needs after the pipeline has run.
#[derive(Debug)]
pub struct StoredMedia {
    pub media_id: Uuid,
    pub storage_key: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: i64,
    pub sha256: String,
    pub exif: Option<serde_json::Value>,
    pub derivatives: Vec<Derivative>,
}

// ---------------------------------------------------------------------------
// format sniffing + decode
// ---------------------------------------------------------------------------

/// HEIC bytes → JPEG bytes (ported from metallian-photos' conversion.rs).
fn convert_heic_to_jpeg(heic_bytes: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
    let decoded = heic::DecoderConfig::new()
        .decode(heic_bytes, heic::PixelLayout::Rgba8)
        .context("Failed to decode the HEIC image")?;
    let buffer: image::RgbaImage =
        image::ImageBuffer::from_raw(decoded.width, decoded.height, decoded.data)
            .ok_or_else(|| anyhow!("Pixel buffer did not match image dimensions"))?;
    let mut jpeg_bytes = Vec::new();
    DynamicImage::from(buffer)
        .into_rgb8()
        .write_to(&mut Cursor::new(&mut jpeg_bytes), image::ImageFormat::Jpeg)
        .context("Unable to convert HEIC to JPEG")?;
    Ok(jpeg_bytes)
}

/// Decode any supported input (JPEG/PNG/WebP natively, HEIC via conversion)
/// into a DynamicImage plus the mime type of the *original* bytes.
pub(crate) fn decode_image(raw: &[u8]) -> Result<(DynamicImage, String), MediaError> {
    // HEIC sniff: ISOBMFF `ftyp` box at offset 4. (HEIC orientation
    // properties are not exposed by the decoder — JPEG is the dominant case.)
    if raw.len() > 12 && &raw[4..8] == b"ftyp" {
        let jpeg = convert_heic_to_jpeg(raw).map_err(|_| MediaError::InvalidImage)?;
        let img = image::load_from_memory(&jpeg).map_err(|_| MediaError::InvalidImage)?;
        return Ok((img, "image/heic".to_string()));
    }

    let format = image::guess_format(raw).map_err(|_| MediaError::InvalidImage)?;
    let mut img = image::load_from_memory(raw).map_err(|_| MediaError::InvalidImage)?;

    // Portrait phone photos carry their rotation in the EXIF Orientation
    // tag; the decoder does not apply it, and our derivatives are
    // EXIF-stripped — so the pipeline must bake it in here, or every
    // served image inherits the wrong orientation forever. (kamadak reads
    // the tag for any EXIF-bearing container.)
    img.apply_orientation(exif_orientation(raw));

    let mime = match format {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err(MediaError::InvalidImage),
    };
    Ok((img, mime.to_string()))
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/heic" => "heic",
        _ => "bin",
    }
}

// ---------------------------------------------------------------------------
// EXIF
// ---------------------------------------------------------------------------

fn exif_string(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    // kamadak's display_value quotes ASCII values; read them raw instead
    let value = match &field.value {
        exif::Value::Ascii(vecs) => vecs.first().map(|v| String::from_utf8_lossy(v).to_string()),
        _ => Some(field.display_value().to_string()),
    };
    value.filter(|s| !s.is_empty())
}

/// GPS rationals → signed decimal degrees, honoring the N/S/E/W ref.
fn gps_decimal(
    exif: &exif::Exif,
    coord: exif::Tag,
    reference: exif::Tag,
    negative_ref: &str,
) -> Option<f64> {
    let field = exif.get_field(coord, exif::In::PRIMARY)?;
    let rationals = match &field.value {
        exif::Value::Rational(r) if r.len() == 3 => r,
        _ => return None,
    };
    let decimal: f64 = rationals
        .iter()
        .map(|r| r.to_f64())
        .collect::<Vec<f64>>()
        .iter()
        .enumerate()
        .map(|(i, v)| v / 60f64.powi(i as i32))
        .sum();
    let sign = match exif.get_field(reference, exif::In::PRIMARY) {
        Some(reference_field) => {
            let reference = reference_field.display_value().to_string();
            if reference.contains(negative_ref) {
                -1.0
            } else {
                1.0
            }
        }
        None => 1.0,
    };
    Some(decimal * sign)
}

/// Pull the display-relevant EXIF fields out of image bytes. Absence of
/// EXIF (common for re-encoded images) yields `None`, not an error.
pub fn extract_exif(raw: &[u8]) -> Option<serde_json::Value> {
    let mut cursor = Cursor::new(raw);
    let exif = exif::Reader::new().read_from_container(&mut cursor).ok()?;

    let mut data = ExifData {
        date_time_original: exif_string(&exif, exif::Tag::DateTimeOriginal),
        make: exif_string(&exif, exif::Tag::Make),
        model: exif_string(&exif, exif::Tag::Model),
        lens_make: exif_string(&exif, exif::Tag::LensMake),
        lens_model: exif_string(&exif, exif::Tag::LensModel),
        exposure_time: exif_string(&exif, exif::Tag::ExposureTime),
        f_number: exif_string(&exif, exif::Tag::FNumber),
        iso: exif_string(&exif, exif::Tag::PhotographicSensitivity),
        focal_length: exif_string(&exif, exif::Tag::FocalLength),
        gps_latitude: gps_decimal(
            &exif,
            exif::Tag::GPSLatitude,
            exif::Tag::GPSLatitudeRef,
            "S",
        ),
        gps_longitude: gps_decimal(
            &exif,
            exif::Tag::GPSLongitude,
            exif::Tag::GPSLongitudeRef,
            "W",
        ),
    };

    // ISO renders as e.g. "64" via display_value; f-number arrives bare
    // (e.g. "8"), so dress it the way the mockup's meta row shows it.
    if let Some(f) = data.f_number.take() {
        data.f_number = Some(format!("ƒ/{f}"));
    }

    let json = serde_json::to_value(&data).ok()?;
    if json.as_object()?.is_empty() {
        None
    } else {
        Some(json)
    }
}

/// The EXIF Orientation tag of the raw bytes, as an image orientation.
pub fn exif_orientation(raw: &[u8]) -> image::metadata::Orientation {
    let mut cursor = Cursor::new(raw);
    exif::Reader::new()
        .read_from_container(&mut cursor)
        .ok()
        .and_then(|exif| {
            exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .cloned()
        })
        .and_then(|field| match &field.value {
            exif::Value::Short(values) => values.first().map(|v| *v as u8),
            _ => None,
        })
        .and_then(image::metadata::Orientation::from_exif)
        .unwrap_or(image::metadata::Orientation::NoTransforms)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// derivatives + pipeline
// ---------------------------------------------------------------------------

fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    img.to_rgb8()
        .write_with_encoder(encoder)
        .map_err(|e| MediaError::Operation(anyhow!("JPEG encode failed: {e}")))?;
    Ok(out)
}

/// `thumb` is a 300×300 square cover crop (feed grid). `medium`/`large`
/// preserve aspect ratio, cap the long edge, and never upscale.
fn derivative_image(img: &DynamicImage, variant: &str) -> DynamicImage {
    match variant {
        "thumb" => img.resize_to_fill(
            THUMB_EDGE,
            THUMB_EDGE,
            image::imageops::FilterType::Lanczos3,
        ),
        "medium" => {
            let (w, h) = (img.width(), img.height());
            let long = w.max(h);
            if long <= MEDIUM_EDGE {
                img.clone()
            } else {
                let scale = MEDIUM_EDGE as f64 / long as f64;
                img.resize_exact(
                    ((w as f64 * scale).round() as u32).max(1),
                    ((h as f64 * scale).round() as u32).max(1),
                    image::imageops::FilterType::Lanczos3,
                )
            }
        }
        _ => {
            let (w, h) = (img.width(), img.height());
            let long = w.max(h);
            if long <= LARGE_EDGE {
                img.clone()
            } else {
                let scale = LARGE_EDGE as f64 / long as f64;
                img.resize_exact(
                    ((w as f64 * scale).round() as u32).max(1),
                    ((h as f64 * scale).round() as u32).max(1),
                    image::imageops::FilterType::Lanczos3,
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// storage keys
// ---------------------------------------------------------------------------

/// The storage key of a media object: `{owner_id}/{media_id}/{name}`.
/// Ownership is the first segment by design (per-user partitioning): bulk
/// per-user operations become prefix operations, and a future backend could
/// map the segment to a literal per-user bucket without another data
/// migration. Single construction point — the pipeline and the backfill
/// bin both go through here.
pub fn media_key(owner_id: Uuid, media_id: Uuid, name: &str) -> String {
    format!("{owner_id}/{media_id}/{name}")
}

/// True when `storage_key` predates owner partitioning — it does not carry
/// `owner_id` as its first segment and needs the backfill bin
/// (`src/bin/backfill_media_keys.rs`).
pub fn is_legacy_key(owner_id: Uuid, storage_key: &str) -> bool {
    !storage_key.starts_with(&format!("{owner_id}/"))
}

// The pipeline: sniff → decode (HEIC converts to JPEG) → EXIF → sha256 →
/// derivatives (thumb/medium/large, all EXIF-free by construction) → save
/// everything under `{owner_id}/{media_id}/…` keys.
/// Generate and store the derivative set for an image under
/// `{owner_id}/{media_id}/{variant}.jpg`. The image passed in must already
/// be EXIF-corrected — see `decode_image`.
pub(crate) async fn store_derivatives(
    storage: &dyn StorageBackend,
    owner_id: Uuid,
    media_id: Uuid,
    img: &DynamicImage,
) -> Result<Vec<Derivative>, MediaError> {
    let mut derivatives = Vec::new();
    for variant in ["thumb", "medium", "large"] {
        let derived = derivative_image(img, variant);
        let encoded = encode_jpeg(&derived)?;
        let key = media_key(owner_id, media_id, &format!("{variant}.jpg"));
        storage
            .save(&key, Bytes::from(encoded.clone()))
            .await
            .map_err(|e| MediaError::Operation(anyhow!("Failed to store {variant}: {e}")))?;
        derivatives.push(Derivative {
            variant: match variant {
                "thumb" => "thumb",
                "medium" => "medium",
                _ => "large",
            },
            storage_key: key,
            width: derived.width(),
            height: derived.height(),
            size_bytes: encoded.len() as i64,
        });
    }
    Ok(derivatives)
}

pub async fn process_and_store(
    storage: &dyn StorageBackend,
    owner_id: Uuid,
    raw: &[u8],
) -> Result<StoredMedia, MediaError> {
    let (img, mime_type) = decode_image(raw)?;
    let exif = extract_exif(raw);

    let media_id = Uuid::new_v4();
    let (width, height) = (img.width(), img.height());
    let sha256 = sha256_hex(raw);

    let original_key = media_key(
        owner_id,
        media_id,
        &format!("original.{}", extension_for(&mime_type)),
    );
    storage
        .save(&original_key, Bytes::copy_from_slice(raw))
        .await
        .map_err(|e| MediaError::Operation(anyhow!("Failed to store original: {e}")))?;

    let derivatives = store_derivatives(storage, owner_id, media_id, &img).await?;

    Ok(StoredMedia {
        media_id,
        storage_key: original_key,
        mime_type,
        width,
        height,
        size_bytes: raw.len() as i64,
        sha256,
        exif,
        derivatives,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorageBackend;
    use std::collections::HashMap;

    fn test_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img =
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            }));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .unwrap();
        buf
    }

    #[tokio::test]
    async fn pipeline_produces_original_and_three_variants() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = test_jpeg(1600, 1000);

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();

        // Assert
        assert_eq!((1600, 1000), (stored.width, stored.height));
        assert_eq!("image/jpeg", stored.mime_type);
        assert_eq!(3, stored.derivatives.len());
        assert_eq!(64, stored.sha256.len(), "sha256 hex digest");
        assert_eq!(
            media_key(owner, stored.media_id, "original.jpg"),
            stored.storage_key,
            "original lives at the owner-scoped original.jpg key"
        );
        for derivative in &stored.derivatives {
            let bytes = storage.find(&derivative.storage_key).await.unwrap();
            assert_eq!(
                derivative.size_bytes as usize,
                bytes.len(),
                "recorded size matches stored bytes"
            );
        }
    }

    #[tokio::test]
    async fn derivative_dimensions_follow_the_variant_contract() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = test_jpeg(1600, 1000);

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();
        let by_variant: HashMap<&str, (u32, u32)> = stored
            .derivatives
            .iter()
            .map(|d| (d.variant, (d.width, d.height)))
            .collect();

        // Assert — thumb crops to a square; medium/large cap the long edge
        assert_eq!((THUMB_EDGE, THUMB_EDGE), by_variant["thumb"]);
        assert_eq!(
            (MEDIUM_EDGE, 400),
            by_variant["medium"],
            "1600x1000 → 640x400"
        );
        assert_eq!(
            (LARGE_EDGE, 675),
            by_variant["large"],
            "1600x1000 → 1080x675"
        );
    }

    #[tokio::test]
    async fn small_sources_are_never_upscaled_except_thumb() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = test_jpeg(400, 300);

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();
        let by_variant: HashMap<&str, (u32, u32)> = stored
            .derivatives
            .iter()
            .map(|d| (d.variant, (d.width, d.height)))
            .collect();

        // Assert — medium/large stay at source size; thumb always fills 300²
        assert_eq!((400, 300), by_variant["medium"]);
        assert_eq!((400, 300), by_variant["large"]);
        assert_eq!((THUMB_EDGE, THUMB_EDGE), by_variant["thumb"]);
    }

    #[tokio::test]
    async fn derivatives_are_exif_free() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = test_jpeg(800, 600);

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();

        // Assert — no derivative contains an EXIF APP1 segment marker
        for derivative in &stored.derivatives {
            let bytes = storage.find(&derivative.storage_key).await.unwrap();
            assert!(
                !bytes.windows(6).any(|window| window == b"Exif\0\0"),
                "{} must be EXIF-free",
                derivative.variant
            );
        }
    }

    #[tokio::test]
    async fn exif_orientation_is_applied_by_the_pipeline() {
        // Arrange — a landscape 1600x1000 JPEG whose EXIF says "rotate 90 CW"
        // (orientation 6: the classic portrait phone photo)
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = jpeg_with_exif_app1(test_jpeg(1600, 1000), exif_tiff(Some("Test"), Some(6)));

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();

        // Assert — dimensions are swapped: the stored image is portrait
        assert_eq!((1000, 1600), (stored.width, stored.height));
        let medium = stored
            .derivatives
            .iter()
            .find(|d| d.variant == "medium")
            .unwrap();
        assert_eq!((400, 640), (medium.width, medium.height), "portrait medium");
        // EXIF (including the orientation tag itself) survives in the DB copy
        assert!(stored.exif.is_some());
    }

    #[tokio::test]
    async fn corrupt_input_yields_invalid_image() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();

        // Act
        let result = process_and_store(&storage, owner, b"definitely not an image").await;

        // Assert
        assert!(matches!(result, Err(MediaError::InvalidImage)));
    }

    #[tokio::test]
    async fn heic_input_converts_through_the_pipeline() {
        // Arrange — real HEIC fixture borrowed from metallian-photos
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = include_bytes!("../../tests/fixtures/IMG_2215.HEIC");

        // Act
        let stored = process_and_store(&storage, owner, raw).await.unwrap();

        // Assert — original kept as HEIC; derivatives are working JPEGs
        assert_eq!("image/heic", stored.mime_type);
        assert_eq!(3, stored.derivatives.len());
        assert!(stored.width > 0 && stored.height > 0);
    }

    #[tokio::test]
    async fn keys_are_owner_scoped() {
        // Arrange
        let storage = InMemoryStorageBackend::new();
        let owner = Uuid::new_v4();
        let raw = test_jpeg(1600, 1000);

        // Act
        let stored = process_and_store(&storage, owner, &raw).await.unwrap();

        // Assert — the original and every derivative live under
        // {owner_id}/{media_id}/… (per-user partitioning, pi-brain 81156523)
        let prefix = format!("{owner}/");
        assert!(
            storage.find(&stored.storage_key).await.is_ok(),
            "original is stored at the owner-scoped key {}",
            stored.storage_key
        );
        for derivative in &stored.derivatives {
            assert!(
                derivative.storage_key.starts_with(&prefix),
                "{} must be owner-scoped",
                derivative.storage_key
            );
            assert!(storage.find(&derivative.storage_key).await.is_ok());
        }
    }

    #[test]
    fn media_key_places_ownership_first() {
        // Arrange
        let owner = Uuid::new_v4();
        let media = Uuid::new_v4();

        // Act
        let key = media_key(owner, media, "original.jpg");

        // Assert — ownership is the first segment: bulk per-user operations
        // become prefix operations
        assert_eq!(format!("{owner}/{media}/original.jpg"), key);
    }

    #[test]
    fn legacy_keys_are_those_without_the_owning_owner_segment() {
        // Arrange
        let owner = Uuid::new_v4();
        let media = Uuid::new_v4();
        let other = Uuid::new_v4();

        // Act / Assert
        assert!(
            is_legacy_key(owner, &format!("{media}/original.jpg")),
            "pre-partitioning keys carry no owner segment"
        );
        assert!(!is_legacy_key(
            owner,
            &media_key(owner, media, "original.jpg")
        ));
        assert!(
            is_legacy_key(owner, &media_key(other, media, "original.jpg")),
            "a key scoped to a different owner is still legacy for this owner"
        );
    }

    /// Hand-built minimal EXIF: JPEG SOI + APP1 carrying a little-endian
    /// TIFF whose IFD0 holds exactly one entry — Make = "Test".
    fn jpeg_with_exif_make(make: &str) -> Vec<u8> {
        let tiff = {
            let mut tiff = Vec::new();
            tiff.extend_from_slice(b"II"); // little-endian
            tiff.extend_from_slice(&0x002A_u16.to_le_bytes()); // TIFF magic
            tiff.extend_from_slice(&8_u32.to_le_bytes()); // IFD0 at offset 8
            tiff.extend_from_slice(&1_u16.to_le_bytes()); // one entry
            tiff.extend_from_slice(&0x010F_u16.to_le_bytes()); // Make
            tiff.extend_from_slice(&2_u16.to_le_bytes()); // ASCII
            tiff.extend_from_slice(&(make.len() as u32 + 1).to_le_bytes()); // count incl. NUL
            tiff.extend_from_slice(&26_u32.to_le_bytes()); // value offset
            tiff.extend_from_slice(&0_u32.to_le_bytes()); // next IFD: none
            tiff.extend_from_slice(make.as_bytes());
            tiff.push(0);
            tiff
        };
        let mut app1_payload = b"Exif\0\0".to_vec();
        app1_payload.extend_from_slice(&tiff);
        let mut jpeg = vec![0xFF, 0xD8]; // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
        jpeg.extend_from_slice(&((app1_payload.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1_payload);
        jpeg
    }

    /// TIFF payload with a Make entry and/or an Orientation entry.
    fn exif_tiff(make: Option<&str>, orientation: Option<u16>) -> Vec<u8> {
        let mut entries: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
        if let Some(make) = make {
            let mut value = make.as_bytes().to_vec();
            value.push(0);
            entries.push((0x010F, 2, value.len() as u32, value)); // ASCII
        }
        if let Some(orientation) = orientation {
            // SHORT fits inline, left-justified in the 4-byte value field
            entries.push((0x0112, 3, 1, vec![orientation as u8, 0, 0, 0]));
        }
        entries.sort_by_key(|entry| entry.0); // TIFF requires ascending tags

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&0x002A_u16.to_le_bytes());
        tiff.extend_from_slice(&8_u32.to_le_bytes());
        tiff.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut data_block = Vec::new();
        for (tag, kind, count, value) in &entries {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&kind.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                let mut inline = value.clone();
                inline.resize(4, 0);
                tiff.extend_from_slice(&inline);
            } else {
                let offset = 8 + 2 + 12 * entries.len() as u32 + 4 + data_block.len() as u32;
                tiff.extend_from_slice(&offset.to_le_bytes());
                data_block.extend_from_slice(value);
            }
        }
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend_from_slice(&data_block);
        tiff
    }

    /// Inject an EXIF APP1 segment into a real JPEG (after the SOI), so
    /// the result both decodes as an image and carries EXIF.
    fn jpeg_with_exif_app1(jpeg: Vec<u8>, tiff: Vec<u8>) -> Vec<u8> {
        let mut app1_payload = b"Exif\0\0".to_vec();
        app1_payload.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1_payload.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1_payload);
        out.extend_from_slice(&jpeg[2..]); // original data after its SOI
        out
    }

    #[test]
    fn exif_extraction_reads_make_from_hand_built_segment() {
        // Act
        let exif = extract_exif(&jpeg_with_exif_make("Test"));

        // Assert
        let exif = exif.expect("EXIF-bearing input should yield data");
        assert_eq!(exif["make"], "Test");
    }

    #[test]
    fn exif_absent_yields_none() {
        // Act / Assert
        assert!(extract_exif(&test_jpeg(50, 50)).is_none());
    }
}
