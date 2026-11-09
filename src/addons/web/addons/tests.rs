//! The `addons.*` ops' pure parts: launch and segment DTOs, id checks and
//! the labelled launch flags.

use super::*;
use crate::addon::Tone;
use crate::app::state::SavedRoute;

fn addons() -> Addons {
    Addons::new(crate::addons::all()).unwrap()
}

/// A persisted pre-addon route.
fn legacy(proxy: Option<&str>, first_party: bool) -> SavedRoute {
    SavedRoute {
        launch: None,
        legacy: crate::addon::LegacyChoice {
            proxy: proxy.map(str::to_owned),
            first_party,
        },
    }
}

#[test]
fn a_session_launch_is_known_only_when_recorded_and_adopted() {
    let addons = addons();
    assert_eq!(launch_of(&addons, None), (false, None));
    assert_eq!(launch_of(&addons, Some(&legacy(None, false))), (true, None));
    let (known, choice) = launch_of(&addons, Some(&legacy(Some("work"), true)));
    assert!(known);
    let choice = choice.unwrap();
    assert_eq!(
        (choice.addon.as_str(), choice.option.as_str()),
        ("vocdoni", "work")
    );
    assert_eq!(choice.flags, ["first-party"]);
    // No addon of this build adopts it: unknown, never direct (F4).
    let none = Addons::empty();
    assert_eq!(
        launch_of(&none, Some(&legacy(Some("work"), false))),
        (false, None)
    );

    let a = SessionAddons {
        known: true,
        choice: Some(choice),
        segments: vec![(
            "vocdoni",
            Segment {
                id: "proxy".into(),
                text: "⇄ proxy:work".into(),
                tone: Tone::Accent,
                priority: 10,
            },
        )],
    };
    assert_eq!(
        launch_dto(&a),
        json!({"known": true, "choice": {"addon": "vocdoni", "option": "work", "flags": ["first-party"]}})
    );
    assert_eq!(
        segments_dto(&a),
        json!([{"addon": "vocdoni", "id": "proxy", "text": "⇄ proxy:work", "tone": "accent", "priority": 10}])
    );
    assert_eq!(
        launch_dto(&SessionAddons::default()),
        json!({"known": false, "choice": null})
    );
}

#[test]
fn ids_and_addons_are_checked() {
    for good in ["a", "vocdoni-test", "x9", &"a".repeat(64)] {
        assert!(check_id("id", good).is_ok(), "{good}");
    }
    for bad in ["", "A", "a b", "a_b", "../x", "é", &"a".repeat(65)] {
        assert_eq!(
            check_id("id", bad),
            Err("bad request: id: not an id".to_owned())
        );
    }
    let addons = addons();
    assert_eq!(addon_named(&addons, "vocdoni"), Ok("vocdoni"));
    assert_eq!(
        addon_named(&addons, "nope"),
        Err("bad request: addon: no such addon".to_owned())
    );
}

#[test]
fn labelled_flags_are_the_select_flags_the_wizard_shows() {
    let addons = addons();
    let launch = addons.get("vocdoni").unwrap().launch().unwrap();
    assert_eq!(
        labelled_flags(launch),
        [json!({"name": "first-party", "label": "First-party session"})]
    );
}

fn arg(v: Value) -> LaunchArg {
    serde_json::from_value(v).unwrap()
}

#[test]
fn a_v2_launch_is_checked_into_a_select() {
    let addons = addons();
    let select = select_of(
        &addons,
        &arg(json!({"pick": "option", "addon": "vocdoni", "option": "work", "flags": ["first-party"]})),
    )
    .unwrap();
    assert_eq!(
        select.pick,
        Pick::Option {
            addon: "vocdoni".into(),
            option: "work".into()
        }
    );
    assert_eq!(select.flags, ["first-party"]);
    assert_eq!(
        select_of(&addons, &arg(json!({"pick": "default"}))).unwrap(),
        LaunchSelect::default()
    );
    assert_eq!(
        select_of(&addons, &arg(json!({"pick": "direct"})))
            .unwrap()
            .pick,
        Pick::Direct
    );
    for (bad, why) in [
        (
            json!({"pick": "option", "addon": "nope", "option": "w"}),
            "no such addon",
        ),
        (
            json!({"pick": "option", "addon": "vocdoni", "option": ""}),
            "option: 1..=128 bytes",
        ),
        (
            json!({"pick": "option", "addon": "vocdoni"}),
            "addon and option go with \"option\" only",
        ),
        (
            json!({"pick": "direct", "addon": "vocdoni"}),
            "addon and option go with \"option\" only",
        ),
        (
            json!({"pick": "anything"}),
            "pick: default, direct or option",
        ),
        (
            json!({"pick": "default", "flags": ["--first-party"]}),
            "no flag --first-party",
        ),
    ] {
        assert_eq!(
            select_of(&addons, &arg(bad.clone())),
            Err(format!("bad request: launch: {why}")),
            "{bad}"
        );
    }
    let defer: Vec<DeferArg> =
        serde_json::from_value(json!([{"addon": "vocdoni", "id": "kb"}])).unwrap();
    assert_eq!(
        deferred_of(&addons, &defer),
        Ok(vec![("vocdoni".to_owned(), "kb".to_owned())])
    );
    let defer: Vec<DeferArg> =
        serde_json::from_value(json!([{"addon": "vocdoni", "id": "../kb"}])).unwrap();
    assert_eq!(
        deferred_of(&addons, &defer),
        Err("bad request: defer: not an id".to_owned())
    );
}
