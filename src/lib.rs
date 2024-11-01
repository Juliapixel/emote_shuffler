use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    time::Duration,
};

use graphql_client::{GraphQLQuery, Response};
use indicatif::{ProgressBar, ProgressStyle};
use log::debug;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::MissedTickBehavior;
use util::{gen_temp_name, shuffle_slice};

pub mod cli;
pub mod util;

#[derive(Debug, Error)]
pub enum SevenTvGqlError {
    #[error("queried user was not found")]
    UserNotFound,
    #[error("{0:?}")]
    EmoteRenameFailed(Vec<graphql_client::Error>),
    #[error(transparent)]
    RequestError(#[from] reqwest::Error),
}

pub struct SevenTvGqlClient {
    client: reqwest::Client,
    auth_token: String,
}

impl SevenTvGqlClient {
    const ENDPOINT: &str = "https://7tv.io/v3/gql";

    pub fn new(token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            auth_token: token,
        }
    }

    pub async fn get_user_emote_set(
        &self,
        username: impl Into<String>,
    ) -> Result<get_emote_set::GetEmoteSetEmoteSet, SevenTvGqlError> {
        let query = GetUserActiveEmoteSet::build_query(get_user_active_emote_set::Variables {
            username: username.into(),
        });

        let resp = self.client.post(Self::ENDPOINT).json(&query).send().await?;

        let response_body: Response<get_user_active_emote_set::ResponseData> = resp.json().await?;

        let found_users = response_body
            .data
            .ok_or(SevenTvGqlError::UserNotFound)?
            .users;

        let result = found_users
            .into_iter()
            .next()
            .ok_or(SevenTvGqlError::UserNotFound)?;

        if result.username.to_lowercase() != query.variables.username.to_lowercase() {
            return Err(SevenTvGqlError::UserNotFound);
        }

        let set_id = result
            .connections
            .into_iter()
            .find(|c| c.platform == ConnectionPlatform::Twitch)
            .and_then(|s| s.emote_set_id);
        if let Some(set_id) = set_id {
            self.get_emote_set(set_id).await
        } else {
            Err(SevenTvGqlError::UserNotFound)
        }
    }

    pub async fn get_emote_set(
        &self,
        set_id: ObjectID,
    ) -> Result<get_emote_set::GetEmoteSetEmoteSet, SevenTvGqlError> {
        let query = GetEmoteSet::build_query(get_emote_set::Variables { set_id });

        let resp = self.client.post(Self::ENDPOINT).json(&query).send().await?;

        let response_body: Response<get_emote_set::ResponseData> = resp.json().await?;

        Ok(response_body.data.unwrap().emote_set)
    }

    pub async fn rename_emote(
        &self,
        set_id: ObjectID,
        emote_id: ObjectID,
        name: impl Into<String>,
    ) -> Result<(), SevenTvGqlError> {
        let query = EmoteRename::build_query(emote_rename::Variables {
            set_id,
            emote_id,
            name: name.into(),
        });

        let resp = self
            .client
            .post(Self::ENDPOINT)
            .json(&query)
            .header(
                "cookie",
                format!("seventv-auth={}", self.auth_token.as_str()),
            )
            .send()
            .await?;

        let response_body: Response<emote_rename::ResponseData> = resp.json().await?;

        if let Some(errors) = response_body.errors {
            return Err(SevenTvGqlError::EmoteRenameFailed(errors));
        }
        Ok(())
    }

    pub async fn shuffle_set(&self, set_id: ObjectID) -> Result<(), SevenTvGqlError> {
        let set = self.get_emote_set(set_id).await?;

        let mut names: Vec<&str> = set.emotes.iter().map(|e| e.name.as_str()).collect();
        if names.is_empty() {
            return Ok(());
        }
        shuffle_slice(&mut names);

        // target name is key, original name is value
        let map: HashMap<&str, &str> = names
            .iter()
            .zip(&set.emotes)
            .map(|(t, orig)| (*t, orig.name.as_str()))
            .collect();

        // maps original name to id
        let emotes: HashMap<&str, ObjectID> =
            set.emotes.iter().map(|e| (e.name.as_str(), e.id)).collect();

        // (source id, target)
        let mut ops: Vec<(ObjectID, Cow<'_, str>)> = Vec::with_capacity(map.len() + 1);
        let mut renamed = HashSet::<&str>::with_capacity(map.len());

        for name in names {
            if renamed.contains(name) {
                continue;
            }
            let first = name;
            ops.push((*emotes.get(first).unwrap(), Cow::Owned(gen_temp_name(16))));
            let mut cur_target = name;
            loop {
                let original = map.get(cur_target).unwrap();
                if *original == first {
                    ops.push((*emotes.get(first).unwrap(), Cow::Borrowed(cur_target)));
                    break;
                }
                if cur_target == *original {
                    continue;
                }
                ops.push((*emotes.get(original).unwrap(), Cow::Borrowed(cur_target)));
                cur_target = original;
                renamed.insert(original);
            }
        }

        let mut interval = tokio::time::interval(std::time::Duration::from_secs_f64(60.0 / 100.0));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let pb = ProgressBar::new(ops.len() as u64).with_style(
            ProgressStyle::with_template(
                "{spinner} [{pos}/{len}] {bar:30.green/gray} ETA: {eta_precise:>}",
            )
            .unwrap()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        );

        pb.enable_steady_tick(Duration::from_millis(100));

        for (source, target) in ops {
            debug!("renaming {} to {}", source, target);
            interval.tick().await;
            self.rename_emote(set_id, source, target).await?;
            pb.inc(1);
        }
        pb.finish();

        Ok(())
    }
}

type ObjectID = ulid::Ulid;

#[derive(graphql_client::GraphQLQuery)]
#[graphql(
    schema_path = "schemas/seventv.graphql",
    query_path = "src/emote_rename.graphql"
)]
struct EmoteRename;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ConnectionPlatform {
    Twitch,
    Kick,
    Youtube,
    Discord,
}

#[derive(graphql_client::GraphQLQuery)]
#[graphql(
    schema_path = "schemas/seventv.graphql",
    query_path = "src/get_user_active_emote_set.graphql"
)]
struct GetUserActiveEmoteSet;

#[derive(graphql_client::GraphQLQuery)]
#[graphql(
    schema_path = "schemas/seventv.graphql",
    query_path = "src/get_emote_set.graphql",
    response_derives = "Debug"
)]
struct GetEmoteSet;
