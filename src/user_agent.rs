//! Extractor for the `User-Agent` header from requests and validates it against a list of bots.

use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{
        header::{
            HeaderValue,
            USER_AGENT,
        },
        request::Parts,
        StatusCode,
    },
};

/// A list of user agents that are expected from services looking to embed a card with images.
/// This includes services like Discord, Slack, and Twitter.
pub const IMAGE_EMBED_USERAGENTS: [&str; 15] = [
    "facebookexternalhit/1.1",
    "Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/31.0.1650.57 Safari/537.36",
    "Mozilla/5.0 (Windows; U; Windows NT 10.0; en-US; Valve Steam Client/default/1596241936; ) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.117 Safari/537.36",
    "Mozilla/5.0 (Windows; U; Windows NT 10.0; en-US; Valve Steam Client/default/0; ) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.117 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_11_1) AppleWebKit/601.2.4 (KHTML, like Gecko) Version/9.0.1 Safari/601.2.4 facebookexternalhit/1.1 Facebot Twitterbot/1.0",
    "facebookexternalhit/1.1",
    "Mozilla/5.0 (Windows; U; Windows NT 6.1; en-US; Valve Steam FriendsUI Tenfoot/0; ) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/84.0.4147.105 Safari/537.36",
    "Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.10; rv:38.0) Gecko/20100101 Firefox/38.0",
    "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)",
    "TelegramBot (like TwitterBot)",
    "Mozilla/5.0 (compatible; January/1.0; +https://gitlab.insrt.uk/revolt/january)",
    "Synapse (bot; +https://github.com/matrix-org/synapse)",
    "Iframely/1.3.1 (+https://iframely.com/docs/about)",
    "test",
];

/// Extractor that gets the `User-Agent` header from the request and checks if it's in the list of
/// user agents that are expected from services looking to embed a card with images.
///
/// If the value is `None`, it means that it is most likely an actual user who clicked on the link
/// rather than an embed bot.
pub struct RequireEmbed(pub Option<HeaderValue>);

#[async_trait]
impl<S> FromRequestParts<S> for RequireEmbed
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        if let Some(user_agent) = parts.headers.get(USER_AGENT) {
            let agent = user_agent.to_str().unwrap();
            let in_embed_list = IMAGE_EMBED_USERAGENTS.contains(&agent);

            // WhatsApp useragents are weird, we just check for the word to cover all bases.
            let whatsapp_in_agent = agent.contains("WhatsApp/");
            match (in_embed_list, whatsapp_in_agent) {
                (true, _) | (_, true) => Ok(RequireEmbed(Some(user_agent.to_owned()))),
                _ => Ok(RequireEmbed(None)),
            }
        } else {
            Err((StatusCode::BAD_REQUEST, "`User-Agent` header is missing"))
        }
    }
}
