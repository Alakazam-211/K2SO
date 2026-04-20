//! E3 tests for `Delivery` enum + `awareness::routing` resolver +
//! `awareness::roster` query.
//!
//! Routing is a pure function — no I/O, no singletons, no locks.
//! Tests are straightforward value assertions. Roster tests use
//! tempdirs for the filesystem side; liveness tests use the
//! session::registry singleton so there's a `TEST_LOCK` for those.

#![cfg(feature = "session_stream")]

use std::path::PathBuf;
use std::sync::Mutex;

use k2so_core::awareness::{
    self, AgentAddress, AgentInfo, AgentSignal, Delivery, DeliveryPlan,
    Priority, RosterFilter, RosterState, SignalKind, TargetState, WorkspaceId,
};
use k2so_core::session::{registry, SessionId};

static ROSTER_TEST_LOCK: Mutex<()> = Mutex::new(());

fn workspace() -> WorkspaceId {
    WorkspaceId("k2so".into())
}

fn signal_to_bar(delivery: Delivery) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: workspace(),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "hello".into(),
        },
    )
    .with_delivery(delivery)
}

fn signal_broadcast() -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "foo".into(),
        },
        AgentAddress::Broadcast,
        SignalKind::Status {
            text: "heartbeat".into(),
        },
    )
}

// ─────────────────────────────────────────────────────────────────────
// Delivery enum
// ─────────────────────────────────────────────────────────────────────

#[test]
fn delivery_defaults_to_live() {
    assert_eq!(Delivery::default(), Delivery::Live);
}

#[test]
fn delivery_json_round_trip_live() {
    let json = serde_json::to_string(&Delivery::Live).unwrap();
    assert_eq!(json, "\"live\"");
    let decoded: Delivery = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, Delivery::Live);
}

#[test]
fn delivery_json_round_trip_inbox() {
    let json = serde_json::to_string(&Delivery::Inbox).unwrap();
    assert_eq!(json, "\"inbox\"");
    let decoded: Delivery = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, Delivery::Inbox);
}

#[test]
fn agent_signal_new_defaults_to_live_delivery() {
    let s = signal_to_bar(Delivery::default());
    assert_eq!(s.delivery, Delivery::Live);
}

#[test]
fn agent_signal_with_delivery_builder_overrides() {
    let s = signal_to_bar(Delivery::Inbox);
    assert_eq!(s.delivery, Delivery::Inbox);
}

#[test]
fn agent_signal_backwards_compat_decodes_without_delivery_field() {
    // Simulate a pre-Phase-3 signal on the wire (no `delivery` key
    // in the JSON) — serde(default) should populate Live.
    let json = r#"{
        "id": "00000000-0000-0000-0000-000000000001",
        "from": { "scope": "broadcast" },
        "to": { "scope": "broadcast" },
        "kind": { "kind": "status", "data": { "text": "legacy" } },
        "priority": "normal",
        "at": "2020-01-01T00:00:00Z"
    }"#;
    let decoded: AgentSignal = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.delivery, Delivery::Live);
}

#[test]
fn agent_signal_round_trip_preserves_delivery_field() {
    let original = signal_to_bar(Delivery::Inbox);
    let encoded = serde_json::to_string(&original).unwrap();
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.delivery, Delivery::Inbox);
    assert_eq!(decoded, original);
}

// ─────────────────────────────────────────────────────────────────────
// Routing resolver — the four-cell matrix
// ─────────────────────────────────────────────────────────────────────

#[test]
fn live_to_live_target_injects_to_pty() {
    let plan =
        awareness::resolve(&signal_to_bar(Delivery::Live), TargetState::Live);
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(plan.inject_to_pty, "Live+Live must inject");
    assert!(!plan.wake_target, "Live+Live must not wake");
    assert!(!plan.write_to_inbox, "Live+Live must not write inbox");
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn live_to_offline_target_wakes_and_queues() {
    let plan = awareness::resolve(
        &signal_to_bar(Delivery::Live),
        TargetState::Offline,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(!plan.inject_to_pty, "Live+Offline must not inject immediately");
    assert!(plan.wake_target, "Live+Offline must wake");
    assert!(
        !plan.write_to_inbox,
        "Live+Offline uses pending-queue, NOT inbox"
    );
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn inbox_to_live_target_writes_inbox_no_inject() {
    let plan = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Live,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("bar"));
    assert!(!plan.inject_to_pty, "Inbox never injects");
    assert!(!plan.wake_target, "Inbox never wakes");
    assert!(plan.write_to_inbox, "Inbox always writes inbox");
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
}

#[test]
fn inbox_to_offline_target_same_as_inbox_to_live() {
    let live = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Live,
    );
    let offline = awareness::resolve(
        &signal_to_bar(Delivery::Inbox),
        TargetState::Offline,
    );
    assert_eq!(
        live, offline,
        "Inbox is state-insensitive — sender intent is the only input"
    );
}

#[test]
fn audit_channels_fire_in_every_matrix_cell() {
    for delivery in [Delivery::Live, Delivery::Inbox] {
        for state in [TargetState::Live, TargetState::Offline] {
            let plan =
                awareness::resolve(&signal_to_bar(delivery), state);
            assert!(
                plan.publish_to_bus,
                "bus broadcast must fire for {delivery:?} × {state:?}"
            );
            assert!(
                plan.write_activity_feed,
                "activity_feed must fire for {delivery:?} × {state:?}"
            );
        }
    }
}

#[test]
fn broadcast_address_returns_target_agent_none() {
    let plan = awareness::resolve(&signal_broadcast(), TargetState::Live);
    assert!(
        plan.target_agent.is_none(),
        "Broadcast fans out in caller, not resolver"
    );
    assert!(plan.publish_to_bus);
    assert!(plan.write_activity_feed);
    assert!(!plan.inject_to_pty);
    assert!(!plan.write_to_inbox);
}

#[test]
fn resolve_for_agent_ignores_signal_to_field() {
    // The per-agent resolver takes the target name explicitly, so
    // `signal.to` doesn't matter. Useful for fanout from
    // Broadcast/Workspace addresses.
    let plan = awareness::resolve_for_agent(
        &signal_broadcast(),
        Some("specific-target".into()),
        TargetState::Live,
    );
    assert_eq!(plan.target_agent.as_deref(), Some("specific-target"));
    // signal_broadcast() has default Live delivery, so Live+Live path.
    assert!(plan.inject_to_pty);
}

// ─────────────────────────────────────────────────────────────────────
// Roster — filesystem reads
// ─────────────────────────────────────────────────────────────────────

fn tmp_workspace_root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-roster-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join(".k2so/agents")).unwrap();
    path
}

fn mk_agent(root: &PathBuf, name: &str, skill: Option<&str>) {
    let dir = root.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent.md"), "# agent\n").unwrap();
    if let Some(s) = skill {
        std::fs::write(dir.join("SKILL.md"), s).unwrap();
    }
}

#[test]
fn roster_all_known_lists_agents_with_agent_md() {
    let _g = ROSTER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_workspace_root("all-known");
    mk_agent(&root, "alice", Some("alice specializes in X"));
    mk_agent(&root, "bob", None);
    // Directory with NO agent.md — should NOT appear.
    std::fs::create_dir_all(root.join(".k2so/agents/not-an-agent")).unwrap();

    let out = awareness::roster::query(RosterFilter::AllKnown(&root));
    let names: Vec<_> = out.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["alice", "bob"]);
}

#[test]
fn roster_skips_hidden_directories() {
    let _g = ROSTER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_workspace_root("hidden");
    mk_agent(&root, "alice", None);
    // .archive/ is a K2SO convention — previously-archived agents,
    // not live. Make it look like it has agent.md to test the hidden-
    // dir skip, not the agent.md filter.
    let archived = root.join(".k2so/agents/.archive");
    std::fs::create_dir_all(&archived).unwrap();
    std::fs::write(archived.join("agent.md"), "# archived\n").unwrap();

    let out = awareness::roster::query(RosterFilter::AllKnown(&root));
    let names: Vec<_> = out.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["alice"]);
}

#[test]
fn roster_skill_summary_strips_frontmatter_and_truncates() {
    let _g = ROSTER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_workspace_root("skill");
    // Single frontmatter block, then a body long enough to test
    // the 200-char truncation.
    let skill_body = format!(
        "---\ntitle: alice\n---\n\n{}",
        "This is the real body content. ".repeat(10)
    );
    mk_agent(&root, "alice", Some(&skill_body));

    let out = awareness::roster::query(RosterFilter::AllKnown(&root));
    let alice = out.iter().find(|a| a.name == "alice").unwrap();
    assert!(
        !alice.skill_summary.contains("title: alice"),
        "summary should not include frontmatter: {:?}",
        alice.skill_summary
    );
    assert!(alice.skill_summary.starts_with("This is the real body"));
    assert!(
        alice.skill_summary.chars().count() <= 200,
        "summary should truncate at 200 chars, got {}",
        alice.skill_summary.chars().count()
    );
}

#[test]
fn roster_lookup_returns_none_for_unknown_agent() {
    let _g = ROSTER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_workspace_root("lookup-none");
    mk_agent(&root, "alice", None);

    let found = awareness::roster::lookup(&root, "alice");
    assert!(found.is_some());
    let absent = awareness::roster::lookup(&root, "bob");
    assert!(absent.is_none());
}

#[test]
fn roster_marks_live_when_session_registered_with_agent_name() {
    let _g = ROSTER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_workspace_root("live-flag");
    mk_agent(&root, "alice", None);
    mk_agent(&root, "bob", None);

    // Register a session for alice.
    let alice_id = SessionId::new();
    let entry = registry::register(alice_id);
    entry.set_agent_name("alice");

    // Scan should mark alice Live, bob Offline.
    let out = awareness::roster::query(RosterFilter::AllKnown(&root));
    let alice = out.iter().find(|a| a.name == "alice").unwrap();
    let bob = out.iter().find(|a| a.name == "bob").unwrap();
    assert_eq!(alice.state, RosterState::Live);
    assert_eq!(bob.state, RosterState::Offline);

    // LiveInWorkspace filter drops bob.
    let live_only = awareness::roster::query(RosterFilter::LiveInWorkspace(&root));
    let live_names: Vec<_> = live_only.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(live_names, vec!["alice"]);

    // Cleanup — if we don't drop the entry, subsequent parallel
    // tests might see alice live when they shouldn't.
    registry::unregister(&alice_id);
}

#[test]
fn agent_info_json_serializes_cleanly() {
    let info = AgentInfo {
        name: "alice".into(),
        workspace: Some(WorkspaceId("k2so".into())),
        state: RosterState::Live,
        skill_summary: "does things".into(),
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains(r#""state":"live""#), "{json}");
    let decoded: AgentInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn signal_with_priority_and_delivery_together() {
    // Regression: priority and delivery are both optional additive
    // fields; setting both shouldn't interfere.
    let sig = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Broadcast,
        SignalKind::Msg {
            text: "test".into(),
        },
    );
    let sig = AgentSignal {
        priority: Priority::Urgent,
        delivery: Delivery::Inbox,
        ..sig
    };
    let encoded = serde_json::to_string(&sig).unwrap();
    let decoded: AgentSignal = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.priority, Priority::Urgent);
    assert_eq!(decoded.delivery, Delivery::Inbox);
}

#[test]
fn resolve_matrix_as_table() {
    // Compact assertion of every cell in one place — doubles as
    // living documentation of the routing rules.
    let cases: Vec<(Delivery, TargetState, fn(&DeliveryPlan) -> bool)> = vec![
        (Delivery::Live, TargetState::Live, |p| {
            p.inject_to_pty && !p.wake_target && !p.write_to_inbox
        }),
        (Delivery::Live, TargetState::Offline, |p| {
            !p.inject_to_pty && p.wake_target && !p.write_to_inbox
        }),
        (Delivery::Inbox, TargetState::Live, |p| {
            !p.inject_to_pty && !p.wake_target && p.write_to_inbox
        }),
        (Delivery::Inbox, TargetState::Offline, |p| {
            !p.inject_to_pty && !p.wake_target && p.write_to_inbox
        }),
    ];
    for (delivery, state, check) in cases {
        let plan = awareness::resolve(&signal_to_bar(delivery), state);
        assert!(
            check(&plan),
            "matrix cell {:?} × {:?} wrong: {:?}",
            delivery,
            state,
            plan
        );
    }
}
