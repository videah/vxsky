//! HTML templates used to render the meta embed tags for embed cards.

use askama::Template;
use atrium_api::app::bsky::{
    actor::defs::ProfileViewBasic,
    feed::post,
};

/// The HTML template used to present meta embed tags to different services.
#[derive(Template)]
#[template(path = "embed_images.html")]
pub struct ImageEmbed {
    /// The profile of the user who made the post.
    pub profile: ProfileViewBasic,
    /// The base URL of this application, used for links.
    pub base_url: String,
    /// The ATUri of the post, will get passed to the thumbnail rendering endpoint.
    pub aturi: String,
    /// The human clickable link to the post.
    pub post_url: String,
    /// The atproto record for the post, containing the posts content.
    pub record: Box<post::Record>,
}

/// The HTML template used to present meta embed tags to different services.
#[derive(Template)]
#[template(path = "embed_account_gated.html")]
pub struct EmbedAccountGated {
    /// The profile of the user who made the post.
    pub profile: ProfileViewBasic,
    /// The base URL of this application, used for links.
    pub base_url: String,
    /// The human clickable link to the post.
    pub post_url: String,
}
