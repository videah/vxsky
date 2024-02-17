//! Improves multi-image embeds for Bluesky by combining all images into one thumbnail.

mod processing;
mod templates;
mod user_agent;

use std::sync::Arc;

use anyhow::anyhow;
use atrium_api::{
    agent::{
        store::MemorySessionStore,
        AtpAgent,
    },
    app::bsky::{
        embed::images::ViewImage,
        feed::{
            defs::{
                PostView,
                PostViewEmbedEnum::AppBskyEmbedImagesView,
            },
            get_posts,
        },
    },
    com::atproto::identity::resolve_handle,
    records::Record,
};
use atrium_xrpc_client::reqwest::ReqwestClient;
use axum::{
    extract::{
        Path,
        Query,
        State,
    },
    http::{
        header,
        StatusCode,
    },
    response::{
        IntoResponse,
        Redirect,
        Response,
    },
    routing::get,
    Router,
};
use axum_thiserror::ErrorStatus;
use image::DynamicImage;
use log::{
    error,
    info,
};
use rayon::prelude::*;
use serde::Deserialize;
use thiserror::Error;
use tokio::net::TcpListener;

use crate::{
    templates::{
        EmbedAccountGated,
        ImageEmbed,
    },
    user_agent::RequireEmbed,
};

/// The application state passed to each request handler.
#[derive(Clone)]
struct AppState {
    /// The [AtpAgent] used to make requests to the bluesky API, handles authentication and session
    /// management.
    agent: Arc<AtpAgent<MemorySessionStore, ReqwestClient>>,
    /// The HTTP client used to make requests for images.
    http_client: reqwest::Client,
    /// The base URL for where this application is hosted (e.g. "https://vsky.app").
    base_url: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Set up logging and load environment variables from a .env file.
    dotenv::dotenv().ok();
    let env = env_logger::Env::default().filter_or("RUST_LOG", "debug");
    env_logger::init_from_env(env);

    let listener = TcpListener::bind("0.0.0.0:8080").await?;

    let base_url = std::env::var("VXSKY_BASE_URL")
        .map_err(|_| anyhow!("The VXSKY_BASE_URL environment variable is required."))?;

    let state = AppState {
        agent: Arc::new(AtpAgent::new(
            ReqwestClient::new("https://bsky.social"),
            MemorySessionStore::default(),
        )),
        http_client: reqwest::Client::new(),
        base_url,
    };

    // Get Bluesky account credentials for API access.
    let identifier = std::env::var("VXSKY_IDENTIFIER").map_err(|_| {
        anyhow!("The VXSKY_IDENTIFIER environment variable is required, either an email or handle.")
    })?;

    let password = std::env::var("VXSKY_APP_PASSWORD")
        .map_err(|_| anyhow!("The VXSKY_APP_PASSWORD environment variable is required."))?;

    // Authenticate with the bluesky API and store the session.
    state.agent.login(identifier, password).await?;

    let app = Router::new()
        .route("/", get(index_redirect))
        .route("/profile/:identifier/post/:post_id", get(embed_image))
        .route("/render-combined-image.png", get(render_combined_image))
        .route("/gated.png", get(gated_image))
        .with_state(state);

    info!("Listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Error type that defines possible failure states for the handlers in this application.
#[derive(Debug, Error, ErrorStatus)]
enum EmbedError {
    #[error("Failed to retrieve DID from identifier")]
    #[status(StatusCode::BAD_REQUEST)]
    ResolveHandleError,
    #[error("Failed to retrieve post: {0}")]
    #[status(StatusCode::INTERNAL_SERVER_ERROR)]
    PostRetrievalError(#[from] atrium_xrpc::error::Error<get_posts::Error>),
    #[error("The API request was successful but no post was returned")]
    #[status(StatusCode::NO_CONTENT)]
    NoPostInResponse,
    #[error("Post has no images, cannot create thumbnail")]
    #[status(StatusCode::UNPROCESSABLE_ENTITY)]
    PostHasNoImages,
    #[error("The record handler for this post's embeds is not implemented")]
    #[status(StatusCode::NOT_IMPLEMENTED)]
    UnimplementedRecordHandler,
    #[error("An error occurred while generating a combined thumbnail: {0}")]
    #[status(StatusCode::INTERNAL_SERVER_ERROR)]
    ThumbnailProcessingError(#[from] processing::ProcessingError),
    #[error("An error occurred while loading an image: {0}")]
    #[status(StatusCode::INTERNAL_SERVER_ERROR)]
    ThumbnailLoadingError(#[from] image::ImageError),
    #[error("An error occurred while downloading an image: {0}")]
    #[status(StatusCode::INTERNAL_SERVER_ERROR)]
    ThumbnailDownloadError(#[from] reqwest::Error),
}

/// Parameters passed to the combined image thumbnail rendering endpoint to tell it what post it
/// should take the images from.
#[derive(Deserialize)]
pub struct RenderImageParams {
    pub uri: String,
}

/// Handler for taking multiple bluesky post images and combining them into one thumbnail.
///
/// This is its own endpoint rather than being part of the `embed_image` handler because it's
/// necessary to have an actual URL to point to for the image, OpenGraph and Twitter card specs
/// don't support base64 encoded images unfortunately.
async fn render_combined_image(
    params: Query<RenderImageParams>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, EmbedError> {
    let response = state
        .agent
        .api
        .app
        .bsky
        .feed
        .get_posts(get_posts::Parameters {
            uris: vec![params.uri.to_owned()],
        })
        .await?;

    let post = response.posts.first().ok_or(EmbedError::NoPostInResponse)?;
    let embed = post.embed.as_ref().ok_or(EmbedError::PostHasNoImages)?;
    match embed {
        AppBskyEmbedImagesView(view) => {
            let tasks: Vec<_> = view
                .images
                .iter()
                .map(|image| get_thumbnail(&state, image))
                .collect();

            let results = futures::future::join_all(tasks).await;
            let images: Result<Vec<_>, _> = results.into_iter().collect();

            // TODO: If there is just one image, just redirect to the post.
            // if images.len() == 1 {
            //     let post_url = format!("https://bsky.app/profile/{identifier}/post/{post_id}");
            //     return Ok(Redirect::temporary(&post_url));
            // }

            let image = processing::generate_combined_thumbnail(images?)?;
            let bytes = image.to_bytes().to_owned();

            Ok(([(header::CONTENT_TYPE, "image/png")], bytes))
        }
        _ => Err(EmbedError::UnimplementedRecordHandler),
    }
}

/// Utility function to download a thumbnail from the Bluesky CDN using a ViewImage's `thumb` and
/// return a DynamicImage.
async fn get_thumbnail(state: &AppState, image: &ViewImage) -> Result<DynamicImage, EmbedError> {
    let response = state.http_client.get(&image.thumb).send().await?;
    let bytes = response.bytes().await?;
    image::load_from_memory(&bytes).map_err(EmbedError::ThumbnailLoadingError)
}

/// Utility function to get a post from the bluesky API given an ATUri.
async fn get_post(uri: &String, state: &AppState) -> Result<PostView, EmbedError> {
    let response = state
        .agent
        .api
        .app
        .bsky
        .feed
        .get_posts(get_posts::Parameters {
            uris: vec![uri.to_owned()],
        })
        .await?;

    let post = response.posts.first().ok_or(EmbedError::NoPostInResponse)?;

    Ok(post.to_owned())
}

/// Selector for the `embed_image` handler to determine whether to return an HTML page featuring the
/// necessary meta tags for an embed card or to 302 Redirect to the post directly.
enum EmbedRouter {
    /// The request has came from a bot associated with embed cards, so we return an HTML page with
    /// the appropriate meta tags.
    Embed(Box<ImageEmbed>),
    /// The request has come from what we think is a real person, so we 302 Redirect to the post
    /// directly.
    DirectLink(Redirect),
    /// The post is account gated and requires an authenticated account to view, so we return an
    /// HTML page with a different embed card informing people of such.
    AccountGatedEmbed(Box<EmbedAccountGated>),
}

impl IntoResponse for EmbedRouter {
    fn into_response(self) -> Response {
        match self {
            EmbedRouter::Embed(embed) => embed.into_response(),
            EmbedRouter::DirectLink(redirect) => redirect.into_response(),
            EmbedRouter::AccountGatedEmbed(embed) => embed.into_response(),
        }
    }
}

/// Handler that takes the same path as a bluesky post and returns an HTML page with OpenGraph and
/// Twitter card meta tags to be displayed in embed card on various services like Discord and
/// Telegram.
async fn embed_image(
    Path((identifier, post_id)): Path<(String, String)>,
    RequireEmbed(embed_agent): RequireEmbed,
    State(state): State<AppState>,
) -> Result<EmbedRouter, EmbedError> {
    let post_url = format!("https://bsky.app/profile/{identifier}/post/{post_id}");

    // There was no User-Agent header that is associated with embedded, so to speed things up we
    // just immediately return a 403 Redirect rather than presenting any HTML.
    if embed_agent.is_none() {
        let direct_link = EmbedRouter::DirectLink(Redirect::temporary(&post_url));
        return Ok(direct_link);
    }

    let response = state
        .agent
        .api
        .com
        .atproto
        .identity
        .resolve_handle(resolve_handle::Parameters {
            handle: identifier.to_owned(),
        })
        .await
        .map_err(|_| EmbedError::ResolveHandleError)?;

    let aturi = format!("at://{}/app.bsky.feed.post/{post_id}", response.did);

    let view = get_post(&aturi, &state).await?;

    // If the account has a label set to require only authenticated accounts we respect it and
    // return a different embed card informing people of such.
    if let Some(labels) = &view.author.labels {
        if labels
            .par_iter()
            .any(|label| label.val == "!no-unauthenticated")
        {
            let embed = EmbedRouter::AccountGatedEmbed(Box::new(EmbedAccountGated {
                profile: view.author.to_owned(),
                base_url: state.base_url.to_owned(),
                post_url,
            }));
            return Ok(embed);
        }
    }

    let record = match view.record {
        Record::AppBskyFeedPost(record) => record,
        _ => return Err(EmbedError::UnimplementedRecordHandler),
    };

    let embed = EmbedRouter::Embed(Box::new(ImageEmbed {
        profile: view.author.to_owned(),
        base_url: state.base_url.to_owned(),
        aturi,
        post_url,
        record,
    }));

    Ok(embed)
}

/// Basic handler to redirect to the main website from the root path.
async fn index_redirect() -> Redirect {
    Redirect::temporary("https://bsky.app/profile/vxsky.app")
}

/// Handler to serve the image used for the account gated embed card, where a user must be logged in
/// to view the contents of a post.
async fn gated_image() -> impl IntoResponse {
    let image = include_bytes!("../assets/gated.png");
    ([(header::CONTENT_TYPE, "image/png")], image.to_vec())
}
