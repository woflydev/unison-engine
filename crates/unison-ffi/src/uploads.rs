//! Upload size limits and the compression ladder (rules in `unison_core::upload_policy`).

use unison_core::upload_policy as policy;

use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum CompressChoice {
    /// On for free accounts, off with Nitro.
    Automatic,
    On,
    Off,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CompressSettingItem {
    pub choice: CompressChoice,
    /// What `Automatic` currently resolves to for this account.
    pub effective: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ImageStepItem {
    pub max_edge: u32,
    pub quality: f32,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum UploadAction {
    Keep,
    /// Re-encode as JPEG (`output_ext`), trying each step until one is under `target_bytes`.
    Image { steps: Vec<ImageStepItem> },
    /// Export as MP4 (`output_ext`) at each height in turn.
    Video { heights: Vec<u32> },
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct UploadPlan {
    pub action: UploadAction,
    pub limit_bytes: u64,
    pub target_bytes: u64,
    pub output_ext: String,
    /// The file is over the limit (whatever the plan): the shell says so if nothing fits.
    pub over_limit: bool,
    /// Remove GPS data from this photo before it uploads (an option, on by default).
    pub strip_location: bool,
}

#[uniffi::export]
impl UnisonEngine {
    pub fn compress_setting(&self, account_id: String) -> CompressSettingItem {
        let choice = self.inner.local.compress_uploads();
        let premium = self.inner.session(id(&account_id)).and_then(|s| s.store.read().me.as_ref().and_then(|u| u.premium_type));
        CompressSettingItem {
            choice: match choice {
                None => CompressChoice::Automatic,
                Some(true) => CompressChoice::On,
                Some(false) => CompressChoice::Off,
            },
            effective: policy::effective(None, premium),
        }
    }

    pub fn set_compress_choice(&self, choice: CompressChoice) {
        self.inner.local.set_compress_uploads(match choice {
            CompressChoice::Automatic => None,
            CompressChoice::On => Some(true),
            CompressChoice::Off => Some(false),
        });
    }

    /// The limit for a message in `channel_id` and what to do with a file of this type and size.
    pub fn upload_plan(&self, account_id: String, channel_id: String, content_type: String, size_bytes: u64) -> UploadPlan {
        let (premium, tier) = match self.inner.session(id(&account_id)) {
            Some(s) => {
                let st = s.store.read();
                let tier = st.channels.get(&id(&channel_id)).and_then(|c| c.guild_id).and_then(|g| st.guilds.get(&g)).map(|g| g.inline.premium_tier);
                (st.me.as_ref().and_then(|u| u.premium_type), tier)
            }
            None => (None, None),
        };
        let limit = policy::limit_bytes(premium, tier);
        let enabled = policy::effective(self.inner.local.compress_uploads(), premium);
        let (action, ext) = match policy::plan(enabled, self.inner.local.comfort().convert_heic, limit, size_bytes, &content_type) {
            policy::Plan::Keep => (UploadAction::Keep, ""),
            policy::Plan::Image(steps) => (UploadAction::Image { steps: steps.into_iter().map(|s| ImageStepItem { max_edge: s.max_edge, quality: s.quality }).collect() }, "jpg"),
            policy::Plan::Video(heights) => (UploadAction::Video { heights }, "mp4"),
        };
        UploadPlan { action, limit_bytes: limit, target_bytes: policy::target_bytes(limit), output_ext: ext.into(), over_limit: size_bytes > limit,
            strip_location: self.inner.local.comfort().strip_location && unison_core::comfort::can_strip_location(&content_type),
        }
    }
}

/// The file name after a re-encode changed its container.
#[uniffi::export]
pub fn renamed_upload(filename: String, ext: String) -> String {
    policy::renamed(&filename, &ext)
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct VideoAttemptsItem {
    /// Heights to export at, best first (none when the movie is too long for the limit).
    pub heights: Vec<u32>,
    pub video_bps: u64,
    pub feasible: bool,
}

#[uniffi::export]
pub fn video_attempts(duration_secs: f64, source_height: u32, target_bytes: u64) -> VideoAttemptsItem {
    let a = policy::video_attempts(duration_secs, source_height, target_bytes);
    VideoAttemptsItem { heights: a.heights, video_bps: a.video_bps, feasible: a.feasible }
}

/// The steps of an image plan worth trying for a picture this size (see `upload_policy::image_steps`).
#[uniffi::export]
pub fn image_attempts(steps: Vec<ImageStepItem>, width: u32, height: u32, target_bytes: u64) -> Vec<ImageStepItem> {
    let all: Vec<policy::ImageStep> = steps.iter().map(|s| policy::ImageStep { max_edge: s.max_edge, quality: s.quality }).collect();
    policy::image_steps(&all, width, height, target_bytes).into_iter().map(|s| ImageStepItem { max_edge: s.max_edge, quality: s.quality }).collect()
}

/// MD5 of a file's bytes as 32 hex digits: what the shell attaches (`FileUpload.original_md5`) when it changes a file.
#[uniffi::export]
pub fn content_md5(bytes: Vec<u8>) -> String {
    unison_core::rest::md5_hex(&bytes)
}
