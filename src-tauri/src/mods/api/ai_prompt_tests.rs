// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the money prompt is told about the route (`mod_ai_prompt_info`, X7), over the same scripted
//! model `ai.complete`'s tests use. The one property that matters is that the prompt and the call
//! agree: whatever the prompt calls free, the call runs without a limit, and whatever it calls paid,
//! the call asks for a limit first.

use serde_json::json;

use super::ai::{describe, prompt_info, PromptRoute};
use super::test_support::{insert_mod, live_session, serve, ModRow, PlanSpec, TestHost};
use super::*;
use crate::local_slot::EndpointClass;
use crate::mods::ai as money;

const MOD: &str = "acme.chat";
const CLOUD: &[&str] = &["a/cheap", "b/dear"];

/// `cloud` are the Background role's cloud models in order, `local` its local model: the names the
/// prompt tells the person about, whatever the route is doing right now.
fn info(
    route: PromptRoute,
    paid: bool,
    unpriced: bool,
    cloud: &[&str],
    local: Option<&str>,
) -> super::ai::PromptInfo {
    super::ai::PromptInfo {
        route,
        paid,
        unpriced,
        cloud_models: cloud.iter().map(|m| (*m).to_owned()).collect(),
        local_model: local.map(str::to_owned),
        suggested_cap_usd: 1.0,
    }
}

async fn told(host: &TestHost) -> super::ai::PromptInfo {
    prompt_info(host).await.unwrap()
}

/// Every route shape with every class of local address, as the prompt describes it. The cloud arm
/// is paid; only a loopback or private local arm is free; a public or unknown one is unpriced
/// (calls refuse), even beside a cloud arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_route_is_described_by_its_arms() {
    use EndpointClass::{Loopback, PrivateRemote, PublicRemote};
    let host = TestHost::new();
    let model = host.model();
    let check = async |spec: PlanSpec, class: Option<EndpointClass>, want| {
        model.set_plan(spec);
        model.set_class(class);
        assert_eq!(told(&host).await, want, "{spec:?} {class:?}");
    };
    check(
        PlanSpec::None,
        None,
        info(PromptRoute::None, false, false, &[], None),
    )
    .await;
    check(
        PlanSpec::Cloud(CLOUD),
        None,
        info(PromptRoute::Cloud, true, false, CLOUD, None),
    )
    .await;
    check(
        PlanSpec::Power(CLOUD),
        None,
        info(
            PromptRoute::CloudForPower,
            true,
            false,
            CLOUD,
            Some("llama"),
        ),
    )
    .await;
    for class in [Loopback, PrivateRemote] {
        check(
            PlanSpec::Local,
            Some(class),
            info(PromptRoute::Local, false, false, &[], Some("llama")),
        )
        .await;
        check(
            PlanSpec::LocalThenCloud(CLOUD),
            Some(class),
            info(
                PromptRoute::LocalThenCloud,
                true,
                false,
                CLOUD,
                Some("llama"),
            ),
        )
        .await;
    }
    for class in [Some(PublicRemote), None] {
        check(
            PlanSpec::Local,
            class,
            info(PromptRoute::Local, false, true, &[], Some("llama")),
        )
        .await;
        check(
            PlanSpec::LocalThenCloud(CLOUD),
            class,
            info(
                PromptRoute::LocalThenCloud,
                false,
                true,
                CLOUD,
                Some("llama"),
            ),
        )
        .await;
    }
}

/// The address lookup is for a local arm only: a cloud route contacts nothing to be described, and
/// asking is a read, not a spend or a price refresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_local_arm_is_looked_up_and_nothing_is_refreshed() {
    use std::sync::atomic::Ordering;
    let host = TestHost::new();
    let model = host.model();
    model.set_plan(PlanSpec::Cloud(CLOUD));
    told(&host).await;
    assert_eq!(model.class_lookups.load(Ordering::SeqCst), 0);
    model.set_plan(PlanSpec::Local);
    told(&host).await;
    assert_eq!(model.class_lookups.load(Ordering::SeqCst), 1);
    model.set_plan(PlanSpec::Power(CLOUD));
    told(&host).await;
    assert_eq!(model.class_lookups.load(Ordering::SeqCst), 1);
    assert_eq!(model.refreshes(), 0, "describing a route fetches no prices");
    assert_eq!(model.entered(), 0, "and calls no model");
}

/// The page reads these field names; a rename here is a break there.
#[test]
fn the_answer_is_camel_case_with_kebab_routes() {
    let cloud = info(
        PromptRoute::CloudForPower,
        true,
        false,
        &["a/b", "c/d"],
        Some("llama"),
    );
    assert_eq!(
        serde_json::to_value(&cloud).unwrap(),
        json!({
            "route": "cloud-for-power",
            "paid": true,
            "unpriced": false,
            "cloudModels": ["a/b", "c/d"],
            "localModel": "llama",
            "suggestedCapUsd": 1.0,
        })
    );
    for (route, word) in [
        (PromptRoute::None, "none"),
        (PromptRoute::Local, "local"),
        (PromptRoute::Cloud, "cloud"),
        (PromptRoute::LocalThenCloud, "local-then-cloud"),
    ] {
        assert_eq!(serde_json::to_value(route).unwrap(), json!(word));
    }
    let none = describe(None, None);
    assert!(none.cloud_models.is_empty());
    assert_eq!(none.local_model, None);
    assert_eq!(money::SUGGESTED_CAP_USD, 1.0);
}

/// The agreement. For every route the prompt calls free, `ai.complete` with no limit runs; for every
/// one it calls paid, the same call asks for a limit (MOD-2002, permission `ai`) and reaches no
/// model; for every one it calls unpriced, the call refuses for want of a price. So a person is
/// never told a call is free that PM would then charge for, and never asked for a limit that
/// changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_prompt_and_the_call_agree_on_what_costs_money() {
    use EndpointClass::{Loopback, PrivateRemote, PublicRemote};
    let cases: &[(PlanSpec, Option<EndpointClass>)] = &[
        (PlanSpec::Local, Some(Loopback)),
        (PlanSpec::Local, Some(PrivateRemote)),
        (PlanSpec::Local, Some(PublicRemote)),
        (PlanSpec::Local, None),
        (PlanSpec::Cloud(CLOUD), None),
        (PlanSpec::Power(CLOUD), None),
        (PlanSpec::LocalThenCloud(CLOUD), Some(Loopback)),
        (PlanSpec::LocalThenCloud(CLOUD), Some(PublicRemote)),
        (PlanSpec::LocalThenCloud(CLOUD), None),
        (PlanSpec::None, None),
    ];
    for (spec, class) in cases {
        let host = TestHost::new();
        host.sql(|c| insert_mod(c, &ModRow::signed(MOD, "second", &["ai"])));
        serve(&host, MOD);
        let session = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
        host.sql(|c| {
            c.execute(
                "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, 'ai', 'granted')",
                [MOD],
            )
            .unwrap();
            for (m, p, q) in [("a/cheap", 1e-6, 2e-6), ("b/dear", 5e-6, 10e-6)] {
                c.execute(
                    "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
                     VALUES (?1, ?2, ?3, '2026-11-17T00:00:00.000Z')",
                    rusqlite::params![m, p, q],
                )
                .unwrap();
            }
        });
        host.model().set_plan(*spec);
        host.model().set_class(*class);
        host.model()
            .set_answer(super::test_support::Answer::local("ok"));
        let said = told(&host).await;
        let auth = authorise(&host, table::ROWS, &session, "ai.complete").unwrap();
        let args = json!({"messages": [{"role": "user", "content": "hi"}], "maxTokens": 10});
        let outcome = ai::run(&host, &auth, &args, limits::CALL_TIMEOUT).await;
        let label = format!("{spec:?} {class:?} {said:?}");
        match (said.route, said.paid, said.unpriced) {
            (PromptRoute::None, _, _) => {
                assert_eq!(outcome.unwrap_err().code, Code::NoModel, "{label}");
            }
            (_, true, false) => {
                let e = outcome.unwrap_err();
                assert_eq!(
                    (e.code, e.permission),
                    (Code::NeedsPrompt, Some("ai")),
                    "{label}"
                );
                assert_eq!(host.model().entered(), 0, "{label}");
            }
            (_, false, true) => {
                assert_eq!(outcome.unwrap_err().code, Code::PriceUnknown, "{label}");
                assert_eq!(host.model().entered(), 0, "{label}");
            }
            (_, false, false) => {
                assert!(outcome.is_ok(), "{label}: {outcome:?}");
            }
            (_, true, true) => panic!("{label}: paid and unpriced at once"),
        }
    }
}

/// The prompt names the models the route will really use, as the person set them and in the order
/// the call tries them; a blank name (a hand-edited setting) is not a model and is not told. On
/// battery the policy's route has no local arm, but the local model it set aside is still named,
/// so the sentence stays true if the person unplugs.
#[test]
fn the_models_told_are_the_routes_own_names_in_order_with_blanks_left_out() {
    use crate::llm_gateway::{CloudArm, LocalArm, RoutePlan};
    use crate::secret::Secret;
    let cloud = |models: &[&str]| CloudArm {
        key: Secret::from("k".to_string()),
        models: models.iter().map(|m| (*m).to_owned()).collect(),
    };
    let local = |model: &str| LocalArm {
        base_url: "http://127.0.0.1:1".into(),
        model: model.into(),
        token: None,
    };
    let told = |plan: RoutePlan| {
        let said = describe(Some(&plan), Some(EndpointClass::Loopback));
        (said.cloud_models, said.local_model)
    };
    let names = |v: &[&str]| v.iter().map(|m| (*m).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        told(RoutePlan::Cloud(cloud(&["z/last", "", "a/first", "  "]))),
        (names(&["z/last", "a/first"]), None)
    );
    assert_eq!(
        told(RoutePlan::LocalOnly(local("llama"))),
        (names(&[]), Some("llama".to_owned()))
    );
    assert_eq!(
        told(RoutePlan::LocalThenCloud {
            local: local(" "),
            cloud: cloud(&["a/b"]),
        }),
        (names(&["a/b"]), None)
    );
    assert_eq!(
        told(RoutePlan::CloudForPower {
            cloud: cloud(&["a/b", "c/d"]),
            displaced_local_model: "llama".into(),
        }),
        (names(&["a/b", "c/d"]), Some("llama".to_owned()))
    );
}
