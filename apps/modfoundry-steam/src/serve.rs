use modfoundry_platform::steam::{
    ItemUpdate, PublishedItem, SteamAccount, SteamEvent, SteamRequest, UploadStage,
};

/// The Steam calls the helper makes; the real one wraps `steamworks`.
pub trait Workshop {
    fn account(&mut self) -> SteamAccount;
    fn create(&mut self) -> Result<(u64, bool), String>;
    fn submit(
        &mut self,
        update: &ItemUpdate,
        progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<(u64, bool), String>;
    fn delete(&mut self, id: u64) -> Result<(), String>;
    fn my_items(&mut self) -> Result<Vec<PublishedItem>, String>;
}

/// Answers one request, emitting its events in order.
pub fn serve(
    request: &SteamRequest,
    workshop: &mut dyn Workshop,
    emit: &mut dyn FnMut(SteamEvent),
) {
    match request {
        SteamRequest::Account => emit(SteamEvent::Account(workshop.account())),
        SteamRequest::Create => emit(match workshop.create() {
            Ok((id, needs_legal_agreement)) => SteamEvent::Created {
                id,
                needs_legal_agreement,
            },
            Err(message) => SteamEvent::Failed { message },
        }),
        SteamRequest::Update(update) => match check_update(update) {
            Ok(()) => submit_update(update, workshop, emit),
            Err(message) => emit(SteamEvent::Failed { message }),
        },
        SteamRequest::Delete { id } => emit(match workshop.delete(*id) {
            Ok(()) => SteamEvent::Deleted { id: *id },
            Err(message) => SteamEvent::Failed { message },
        }),
        SteamRequest::MyItems => emit(match workshop.my_items() {
            Ok(items) => SteamEvent::Items {
                account: workshop.account(),
                items,
            },
            Err(message) => SteamEvent::Failed { message },
        }),
    }
}

fn check_update(update: &ItemUpdate) -> Result<(), String> {
    if update.id == 0 {
        return Err("no item id to update".to_owned());
    }
    if !update.content_dir.is_dir() {
        return Err(format!(
            "content folder not found: {}",
            update.content_dir.display()
        ));
    }
    if let Some(preview) = update.preview.as_ref().filter(|preview| !preview.is_file()) {
        return Err(format!("preview image not found: {}", preview.display()));
    }
    Ok(())
}

fn submit_update(
    update: &ItemUpdate,
    workshop: &mut dyn Workshop,
    emit: &mut dyn FnMut(SteamEvent),
) {
    let result = workshop.submit(update, &mut |stage, done, total| {
        emit(SteamEvent::Progress { stage, done, total })
    });
    emit(match result {
        Ok((id, needs_legal_agreement)) => SteamEvent::Submitted {
            id,
            needs_legal_agreement,
        },
        Err(message) => SteamEvent::Failed { message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_platform::steam::Visibility;
    use std::path::PathBuf;

    fn account() -> SteamAccount {
        SteamAccount {
            name: "builder".to_owned(),
            steam_id: 76561198000000001,
        }
    }

    struct FakeWorkshop {
        account: SteamAccount,
        create: Result<(u64, bool), String>,
        submit_progress: Vec<(UploadStage, u64, u64)>,
        submit: Result<(u64, bool), String>,
        submits: usize,
        delete: Result<(), String>,
        items: Result<Vec<PublishedItem>, String>,
    }

    impl FakeWorkshop {
        fn new() -> Self {
            Self {
                account: account(),
                create: Ok((7, false)),
                submit_progress: Vec::new(),
                submit: Ok((7, false)),
                submits: 0,
                delete: Ok(()),
                items: Ok(Vec::new()),
            }
        }
    }

    impl Workshop for FakeWorkshop {
        fn account(&mut self) -> SteamAccount {
            self.account.clone()
        }

        fn create(&mut self) -> Result<(u64, bool), String> {
            self.create.clone()
        }

        fn submit(
            &mut self,
            _update: &ItemUpdate,
            progress: &mut dyn FnMut(UploadStage, u64, u64),
        ) -> Result<(u64, bool), String> {
            self.submits += 1;
            for (stage, done, total) in self.submit_progress.clone() {
                progress(stage, done, total);
            }
            self.submit.clone()
        }

        fn delete(&mut self, _id: u64) -> Result<(), String> {
            self.delete.clone()
        }

        fn my_items(&mut self) -> Result<Vec<PublishedItem>, String> {
            self.items.clone()
        }
    }

    fn update(id: u64, content_dir: PathBuf) -> ItemUpdate {
        ItemUpdate {
            id,
            content_dir,
            preview: None,
            title: None,
            description: None,
            visibility: None,
            tags: None,
            remove_keys: Vec::new(),
            kv_tags: Vec::new(),
            change_note: None,
        }
    }

    fn answer(request: &SteamRequest, workshop: &mut dyn Workshop) -> Vec<SteamEvent> {
        let mut events = Vec::new();
        serve(request, workshop, &mut |event| events.push(event));
        events
    }

    #[test]
    fn an_account_request_answers_with_the_account() {
        let mut workshop = FakeWorkshop::new();
        assert_eq!(
            answer(&SteamRequest::Account, &mut workshop),
            vec![SteamEvent::Account(account())]
        );
    }

    #[test]
    fn a_create_request_answers_with_the_new_item_or_the_failure() {
        let cases = [
            (
                Ok((7, true)),
                SteamEvent::Created {
                    id: 7,
                    needs_legal_agreement: true,
                },
            ),
            (
                Err("Steam is not running".to_owned()),
                SteamEvent::Failed {
                    message: "Steam is not running".to_owned(),
                },
            ),
        ];
        for (create, want) in cases {
            let mut workshop = FakeWorkshop::new();
            workshop.create = create;
            assert_eq!(answer(&SteamRequest::Create, &mut workshop), vec![want]);
        }
    }

    #[test]
    fn an_update_that_cannot_be_submitted_fails_without_calling_steam() {
        let missing = std::env::temp_dir().join(format!("mf-steam-missing-{}", std::process::id()));
        let cases = [
            (
                update(0, std::env::temp_dir()),
                "no item id to update".to_owned(),
            ),
            (
                update(7, missing.clone()),
                format!("content folder not found: {}", missing.display()),
            ),
            (
                ItemUpdate {
                    preview: Some(missing.clone()),
                    ..update(7, std::env::temp_dir())
                },
                format!("preview image not found: {}", missing.display()),
            ),
        ];
        for (item, message) in cases {
            let mut workshop = FakeWorkshop::new();
            let events = answer(&SteamRequest::Update(item), &mut workshop);
            assert_eq!(events, vec![SteamEvent::Failed { message }]);
            assert_eq!(workshop.submits, 0);
        }
    }

    #[test]
    fn an_update_reports_its_progress_then_the_submitted_item() {
        let mut workshop = FakeWorkshop::new();
        workshop.submit_progress = vec![
            (UploadStage::UploadingContent, 5, 10),
            (UploadStage::Committing, 10, 10),
        ];
        let request = SteamRequest::Update(update(7, std::env::temp_dir()));
        assert_eq!(
            answer(&request, &mut workshop),
            vec![
                SteamEvent::Progress {
                    stage: UploadStage::UploadingContent,
                    done: 5,
                    total: 10,
                },
                SteamEvent::Progress {
                    stage: UploadStage::Committing,
                    done: 10,
                    total: 10,
                },
                SteamEvent::Submitted {
                    id: 7,
                    needs_legal_agreement: false,
                },
            ]
        );
    }

    #[test]
    fn a_delete_request_answers_with_the_deleted_item_or_the_failure() {
        let cases = [
            (Ok(()), SteamEvent::Deleted { id: 9 }),
            (
                Err("Access Denied".to_owned()),
                SteamEvent::Failed {
                    message: "Access Denied".to_owned(),
                },
            ),
        ];
        for (delete, want) in cases {
            let mut workshop = FakeWorkshop::new();
            workshop.delete = delete;
            assert_eq!(
                answer(&SteamRequest::Delete { id: 9 }, &mut workshop),
                vec![want]
            );
        }
    }

    #[test]
    fn a_my_items_request_answers_with_the_account_and_its_items_or_the_failure() {
        let item = PublishedItem {
            id: 7,
            title: "Gun Pack".to_owned(),
            description: String::new(),
            visibility: Visibility::Public,
            tags: Vec::new(),
            kv_tags: Vec::new(),
            updated: 1_700_000_000,
        };
        let cases = [
            (
                Ok(vec![item.clone()]),
                SteamEvent::Items {
                    account: account(),
                    items: vec![item],
                },
            ),
            (
                Err("Steam did not answer within 60 s".to_owned()),
                SteamEvent::Failed {
                    message: "Steam did not answer within 60 s".to_owned(),
                },
            ),
        ];
        for (items, want) in cases {
            let mut workshop = FakeWorkshop::new();
            workshop.items = items;
            assert_eq!(answer(&SteamRequest::MyItems, &mut workshop), vec![want]);
        }
    }
}
