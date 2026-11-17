//! The real Workshop calls, made through the `steamworks` client.

use crate::serve::Workshop;
use modfoundry_platform::steam::{
    ItemUpdate, KeyValueTag, PublishedItem, SteamAccount, UploadStage, Visibility,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use steamworks::{
    AccountId, AppIDs, AppId, Client, FileType, PublishedFileId, PublishedFileVisibility,
    QueryResults, SteamError, UGCType, UpdateStatus, UserList, UserListOrder,
};

/// DayZ's Steam app id; Workshop items are published under it.
pub const APP_ID: AppId = AppId(221100);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const SUBMIT_STALL_LIMIT: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_millis(100);
const PAGE_SIZE: usize = 50;

trait Clock {
    fn elapsed(&self) -> Duration;
    fn pause(&mut self);
}

struct RealClock(Instant);

impl Clock for RealClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn pause(&mut self) {
        std::thread::sleep(POLL);
    }
}

/// Pumps until the slot holds a value, or until `limit` has passed since the last tick that
/// reported progress. `limit: None` waits for ever.
fn wait_for<T>(
    slot: &Mutex<Option<T>>,
    limit: Option<Duration>,
    clock: &mut dyn Clock,
    mut pump: impl FnMut(),
    mut on_tick: impl FnMut() -> bool,
) -> Option<T> {
    let mut idle_from = Duration::ZERO;
    loop {
        pump();
        if let Some(value) = slot.lock().unwrap().take() {
            return Some(value);
        }
        if limit.is_some_and(|limit| clock.elapsed() - idle_from >= limit) {
            return None;
        }
        if on_tick() {
            idle_from = clock.elapsed();
        }
        clock.pause();
    }
}

/// The Workshop calls over a running Steam client.
pub struct SteamWorkshop {
    client: Client,
}

impl SteamWorkshop {
    /// Wraps a client that has run `init_app`.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn call<T: Send + 'static>(
        &self,
        timeout: Option<Duration>,
        start: impl FnOnce(Box<dyn FnOnce(T) + Send>),
    ) -> Result<T, String> {
        let slot = Arc::new(Mutex::new(None));
        let filled = Arc::clone(&slot);
        start(Box::new(move |value| *filled.lock().unwrap() = Some(value)));
        self.wait(&slot, timeout, || false).ok_or_else(|| {
            format!(
                "Steam did not answer within {} s",
                timeout.unwrap_or_default().as_secs()
            )
        })
    }

    fn wait<T>(
        &self,
        slot: &Mutex<Option<T>>,
        limit: Option<Duration>,
        on_tick: impl FnMut() -> bool,
    ) -> Option<T> {
        wait_for(
            slot,
            limit,
            &mut RealClock(Instant::now()),
            || self.client.run_callbacks(),
            on_tick,
        )
    }

    /// One page of the account's items, with the number Steam returned for it.
    fn items_page(
        &self,
        account: AccountId,
        page: u32,
    ) -> Result<(u32, Vec<PublishedItem>), String> {
        let query = self
            .client
            .ugc()
            .query_user(
                account,
                UserList::Published,
                UGCType::Items,
                UserListOrder::LastUpdatedDesc,
                AppIDs::Both {
                    creator: APP_ID,
                    consumer: APP_ID,
                },
                page,
            )
            .map_err(|e| e.to_string())?;
        self.call(Some(CALL_TIMEOUT), |done| {
            query
                .include_long_desc(true)
                .include_key_value_tags(true)
                .fetch(move |results| {
                    done(
                        results
                            .map(|results| (results.returned_results(), read_items(&results)))
                            .map_err(|e| e.to_string()),
                    )
                })
        })?
    }
}

impl Workshop for SteamWorkshop {
    fn account(&mut self) -> SteamAccount {
        SteamAccount {
            name: self.client.friends().name(),
            steam_id: self.client.user().steam_id().raw(),
        }
    }

    fn create(&mut self) -> Result<(u64, bool), String> {
        let ugc = self.client.ugc();
        let answer: Result<(PublishedFileId, bool), SteamError> = self
            .call(Some(CALL_TIMEOUT), |done| {
                ugc.create_item(APP_ID, FileType::Community, done)
            })?;
        answer
            .map(|(id, legal)| (id.0, legal))
            .map_err(|e| e.to_string())
    }

    fn submit(
        &mut self,
        update: &ItemUpdate,
        progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<(u64, bool), String> {
        let mut handle = self
            .client
            .ugc()
            .start_item_update(APP_ID, PublishedFileId(update.id))
            .content_path(&update.content_dir);
        if let Some(preview) = &update.preview {
            handle = handle.preview_path(preview);
        }
        if let Some(title) = &update.title {
            handle = handle.title(title);
        }
        if let Some(description) = &update.description {
            handle = handle.description(description);
        }
        if let Some(visibility) = update.visibility {
            handle = handle.visibility(to_steam_visibility(visibility));
        }
        if let Some(tags) = &update.tags {
            handle = handle.tags(tags.clone(), false);
        }
        for key in &update.remove_keys {
            handle = handle.remove_key_value_tag(key);
        }
        for tag in &update.kv_tags {
            handle = handle.add_key_value_tag(&tag.key, &tag.value);
        }

        let slot = Arc::new(Mutex::new(None));
        let filled = Arc::clone(&slot);
        let watch = handle.submit(update.change_note.as_deref(), move |result| {
            *filled.lock().unwrap() = Some(result)
        });
        let mut last = None;
        let answer: Result<(PublishedFileId, bool), SteamError> = self
            .wait(&slot, Some(SUBMIT_STALL_LIMIT), || {
                let state = watch.progress();
                if last == Some(state) {
                    return false;
                }
                last = Some(state);
                if let Some(stage) = upload_stage(state.0) {
                    progress(stage, state.1, state.2);
                }
                true
            })
            .ok_or_else(|| {
                format!(
                    "Steam stopped answering the upload: no progress for {} s",
                    SUBMIT_STALL_LIMIT.as_secs()
                )
            })?;
        answer
            .map(|(id, legal)| (id.0, legal))
            .map_err(upload_failure)
    }

    fn delete(&mut self, id: u64) -> Result<(), String> {
        let ugc = self.client.ugc();
        let answer: Result<(), SteamError> = self.call(Some(CALL_TIMEOUT), |done| {
            ugc.delete_item(PublishedFileId(id), done)
        })?;
        answer.map_err(|e| e.to_string())
    }

    fn my_items(&mut self) -> Result<Vec<PublishedItem>, String> {
        let account = self.client.user().steam_id().account_id();
        let mut items = Vec::new();
        let mut page = 1;
        loop {
            let (returned, batch) = self.items_page(account, page)?;
            let last = (returned as usize) < PAGE_SIZE;
            items.extend(batch);
            if last {
                return Ok(items);
            }
            page += 1;
        }
    }
}

fn read_items(results: &QueryResults) -> Vec<PublishedItem> {
    (0..results.returned_results())
        .filter_map(|index| {
            let item = results.get(index)?;
            let kv_tags = (0..results.key_value_tags(index))
                .filter_map(|tag| results.get_key_value_tag(index, tag))
                .map(|(key, value)| KeyValueTag { key, value })
                .collect();
            Some(PublishedItem {
                id: item.published_file_id.0,
                title: item.title,
                description: item.description,
                visibility: from_steam_visibility(item.visibility),
                tags: item
                    .tags
                    .into_iter()
                    .filter(|tag| !tag.is_empty())
                    .collect(),
                kv_tags,
                updated: item.time_updated,
            })
        })
        .collect()
}

fn to_steam_visibility(visibility: Visibility) -> PublishedFileVisibility {
    match visibility {
        Visibility::Public => PublishedFileVisibility::Public,
        Visibility::FriendsOnly => PublishedFileVisibility::FriendsOnly,
        Visibility::Private => PublishedFileVisibility::Private,
        Visibility::Unlisted => PublishedFileVisibility::Unlisted,
    }
}

fn from_steam_visibility(visibility: PublishedFileVisibility) -> Visibility {
    match visibility {
        PublishedFileVisibility::Public => Visibility::Public,
        PublishedFileVisibility::FriendsOnly => Visibility::FriendsOnly,
        PublishedFileVisibility::Private => Visibility::Private,
        PublishedFileVisibility::Unlisted => Visibility::Unlisted,
    }
}

fn upload_stage(status: UpdateStatus) -> Option<UploadStage> {
    match status {
        UpdateStatus::Invalid => None,
        UpdateStatus::PreparingConfig => Some(UploadStage::PreparingConfig),
        UpdateStatus::PreparingContent => Some(UploadStage::PreparingContent),
        UpdateStatus::UploadingContent => Some(UploadStage::UploadingContent),
        UpdateStatus::UploadingPreviewFile => Some(UploadStage::UploadingPreview),
        UpdateStatus::CommittingChanges => Some(UploadStage::Committing),
    }
}

/// Steam answers a submit with its generic failure when it closes mid-upload.
fn upload_failure(error: SteamError) -> String {
    match error {
        SteamError::Generic => format!("{error}; is Steam still running?"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock(Duration);

    impl Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.0
        }

        fn pause(&mut self) {
            self.0 += Duration::from_secs(1);
        }
    }

    #[test]
    fn an_upload_that_fails_generically_asks_whether_steam_still_runs() {
        let generic = upload_failure(SteamError::Generic);
        assert!(generic.contains("is Steam still running?"), "{generic}");
        let other = upload_failure(SteamError::AccessDenied);
        assert_eq!(other, SteamError::AccessDenied.to_string());
    }

    #[test]
    fn wait_for_takes_the_value_as_soon_as_the_pump_fills_the_slot() {
        let cases = [
            (None, 3, "no limit"),
            (None, 500, "no limit, slow"),
            (Some(Duration::from_secs(10)), 50, "a live upload"),
        ];
        for (limit, fill_at, label) in cases {
            let slot = Mutex::new(None);
            let mut clock = FakeClock(Duration::ZERO);
            let mut pumps = 0;
            let value = wait_for(
                &slot,
                limit,
                &mut clock,
                || {
                    pumps += 1;
                    if pumps == fill_at {
                        *slot.lock().unwrap() = Some(7u32);
                    }
                },
                || true,
            );
            assert_eq!(value, Some(7), "{label}");
            assert_eq!(pumps, fill_at, "{label}");
        }
    }

    #[test]
    fn the_visibility_mapping_is_a_bijection() {
        let cases = [
            Visibility::Public,
            Visibility::FriendsOnly,
            Visibility::Private,
            Visibility::Unlisted,
        ];
        for visibility in cases {
            assert_eq!(
                from_steam_visibility(to_steam_visibility(visibility)),
                visibility,
                "{visibility:?}"
            );
        }
        assert_eq!(
            from_steam_visibility(PublishedFileVisibility::Unlisted),
            Visibility::Unlisted
        );
    }

    #[test]
    fn wait_for_stops_at_the_limit_unless_the_ticks_report_progress() {
        let cases = [
            ("a silent upload", 0, 10..=12),
            ("an upload that reported progress for a while", 30, 38..=42),
        ];
        for (label, ticking, pumps_wanted) in cases {
            let slot = Mutex::new(None);
            let mut clock = FakeClock(Duration::ZERO);
            let mut pumps = 0;
            let mut ticks = 0;
            let value: Option<u32> = wait_for(
                &slot,
                Some(Duration::from_secs(10)),
                &mut clock,
                || pumps += 1,
                || {
                    ticks += 1;
                    ticks <= ticking
                },
            );
            assert_eq!(value, None, "{label}");
            assert!(pumps_wanted.contains(&pumps), "{label}: {pumps} pumps");
        }
    }
}
