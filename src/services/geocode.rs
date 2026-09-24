// src/services/geocode.rs

// dependencies
use serde_json::Value;

/// Reverse geocoding: GPS coordinates → a displayable location name.
///
/// Privacy note: this sends coordinates to the configured Nominatim
/// (OpenStreetMap) service at upload time, best-effort, and degrades to
/// no location when unavailable or disabled. Coordinates are never sent
/// anywhere else, and served images remain EXIF-free.
pub struct Geocoder {
    client: reqwest::Client,
    /// `None` when disabled.
    base_url: Option<String>,
}

impl Geocoder {
    pub fn new(enabled: bool, base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("halation/0.x (self-hosted photo server)")
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: enabled.then_some(base_url),
        }
    }

    /// Whether reverse geocoding is enabled (test injection point).
    pub fn enabled(&self) -> bool {
        self.base_url.is_some()
    }

    /// Best-effort reverse geocode. `None` = disabled, unreachable, or no
    /// usable address — never an error surfaced to the user.
    pub async fn reverse(&self, lat: f64, lng: f64) -> Option<String> {
        let base_url = self.base_url.as_ref()?;
        let response = self
            .client
            .get(format!("{base_url}/reverse"))
            .query(&vec![
                ("lat".to_string(), lat.to_string()),
                ("lon".to_string(), lng.to_string()),
                ("format".to_string(), "jsonv2".to_string()),
                ("zoom".to_string(), "16".to_string()),
            ])
            .send()
            .await
            .ok()?;
        let json: Value = response.json().await.ok()?;
        parse_nominatim_location(&json)
    }
}

/// Chip-style location from a Nominatim `address` object: the most
/// specific place name available, in mockup-chip spirit.
pub fn parse_nominatim_location(json: &Value) -> Option<String> {
    let address = json.get("address")?;
    for key in [
        "neighbourhood",
        "suburb",
        "village",
        "town",
        "hamlet",
        "city",
        "county",
    ] {
        if let Some(name) = address.get(key).and_then(|v| v.as_str())
            && !name.is_empty() {
                return Some(name.to_string());
            }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_neighbourhood_over_city() {
        // Arrange — a Nominatim-shaped response
        let json = json!({
            "place_id": 1,
            "address": {
                "neighbourhood": "Sunset Hill",
                "suburb": "Ballard",
                "city": "Seattle",
                "county": "King County",
                "state": "Washington"
            },
            "display_name": "Sunset Hill, Ballard, Seattle, ..."
        });

        // Act
        let location = parse_nominatim_location(&json);

        // Assert — most specific name wins
        assert_eq!(location.as_deref(), Some("Sunset Hill"));
    }

    #[test]
    fn falls_back_to_city_when_no_sublocality() {
        // Arrange
        let json = json!({ "address": { "city": "Port Townsend", "state": "Washington" } });

        // Act / Assert
        assert_eq!(parse_nominatim_location(&json).as_deref(), Some("Port Townsend"));
    }

    #[test]
    fn addressless_response_yields_none() {
        assert!(parse_nominatim_location(&json!({ "error": "no result" })).is_none());
    }
}
