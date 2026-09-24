// src/routes/feed.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::posts::{FEED_PAGE_SIZE, PostCard};
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, web};
use datastar::actix::Sse;
use datastar::prelude::{ElementPatchMode, PatchElements};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    /// Cursor: the id of the last post currently on the page.
    pub before: Option<String>,
}

fn render_cards(templates: &dyn TemplateRenderer, cards: &[PostCard]) -> Result<String, Error> {
    let mut html = String::new();
    for card in cards {
        html.push_str(&templates.render(
            "partials/post_card.html",
            &serde_json::json!({ "post": card }),
        )?);
    }
    Ok(html)
}

fn load_more_button(
    templates: &dyn TemplateRenderer,
    before: Option<String>,
) -> Result<String, Error> {
    let context = serde_json::json!({
        "load_more_before": before.map(|id| id.to_string()),
    });
    Ok(templates.render("partials/load_more.html", &context)?)
}

async fn render_feed_page(
    templates: &dyn TemplateRenderer,
    db: &dyn DatabaseBackend,
    before: Option<Uuid>,
    identity: Option<Identity>,
    title: &'static str,
    heading: &str,
) -> Result<HttpResponse, Error> {
    let (cards, has_more) = db
        .load_recent_page(before, FEED_PAGE_SIZE)
        .await
        .map_err(crate::utils::e500)?;
    let load_more_before = if has_more {
        cards.last().map(|card| card.id.to_string())
    } else {
        None
    };

    let context = serde_json::json!({
        "title": title,
        "header": "Halation",
        "sub_header": heading,
        "logged_in": identity.is_some(),
        "posts": cards,
        "load_more_before": load_more_before,
    });

    let body = templates.render("feed.html", &context)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

/// GET / — the feed. Phase 3 note: follows do not exist yet, so this
/// serves the global timeline; Phase 5 switches it to following-only.
pub async fn get_feed(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    query: web::Query<FeedQuery>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let before = query.before.as_ref().and_then(|s| Uuid::parse_str(s).ok());
    render_feed_page(
        templates.get_ref().as_ref(),
        db.get_ref().as_ref(),
        before,
        identity,
        "Feed",
        "The golden glow around the highlights.",
    )
    .await
}

/// GET /recent — global chronological firehose. Honest by construction:
/// newest first, no ranking, no surprises.
pub async fn get_recent(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    query: web::Query<FeedQuery>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let before = query.before.as_ref().and_then(|s| Uuid::parse_str(s).ok());
    render_feed_page(
        templates.get_ref().as_ref(),
        db.get_ref().as_ref(),
        before,
        identity,
        "Recent",
        "Everything, newest first.",
    )
    .await
}

/// GET /hashtags/{tag} — posts carrying the tag.
pub async fn get_hashtag_feed(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let tag = path.into_inner().to_lowercase();
    let (cards, _has_more) = db
        .load_tag_page(&tag, None, FEED_PAGE_SIZE)
        .await
        .map_err(crate::utils::e500)?;

    let context = serde_json::json!({
        "title": format!("#{tag}"),
        "header": "Halation",
        "sub_header": format!("Posts tagged #{tag}"),
        "logged_in": identity.is_some(),
        "tag": tag,
        "posts": cards,
    });

    let body = templates.render("hashtag.html", &context)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

/// GET /fragments/feed?before={post_id} — load-more fragment. Returns a
/// Datastar SSE stream with two patches: the next page of post cards
/// inserted before the #load-more button, then the button replaced with
/// the next cursor — or removed once the feed is exhausted.
pub async fn feed_fragment(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    query: web::Query<FeedQuery>,
) -> Result<Sse, Error> {
    let before = query.before.as_ref().and_then(|s| Uuid::parse_str(s).ok());
    let (cards, has_more) = db
        .load_recent_page(before, FEED_PAGE_SIZE)
        .await
        .map_err(crate::utils::e500)?;

    let cards_html = render_cards(templates.get_ref().as_ref(), &cards)?;
    let mut events = vec![
        PatchElements::new(cards_html)
            .selector("#load-more")
            .mode(ElementPatchMode::Before)
            .into_datastar_event(),
    ];

    if has_more {
        let next_cursor = cards.last().map(|card| card.id.to_string());
        let button_html = load_more_button(templates.get_ref().as_ref(), next_cursor)?;
        events.push(
            PatchElements::new(button_html)
                .selector("#load-more")
                .mode(ElementPatchMode::Replace)
                .into_datastar_event(),
        );
    } else {
        events.push(PatchElements::new_remove("#load-more").into_datastar_event());
    }

    Ok(Sse::new(tokio_stream::iter(events)))
}
