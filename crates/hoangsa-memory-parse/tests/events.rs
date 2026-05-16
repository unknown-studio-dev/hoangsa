//! Integration tests for event-bus call-site extraction
//! (`SymbolTable::events`). Drives the public `parse_file` path so the
//! whole walker + per-language matcher is exercised together.

use hoangsa_memory_parse::{EventEdge, EventRole, LanguageRegistry, SymbolTable, parse_file};

#[derive(Debug, Clone)]
struct Captured {
    role: EventRole,
    topic: String,
    topic_expr: Option<String>,
    bus: Option<String>,
    handler: Option<String>,
    owner: String,
}

async fn parse_src(filename: &str, src: &str) -> SymbolTable {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(filename);
    tokio::fs::write(&path, src).await.expect("write");
    let registry = LanguageRegistry::new();
    let (_chunks, table) = parse_file(&registry, &path).await.expect("parse");
    table
}

fn to_captured(events: Vec<EventEdge>) -> Vec<Captured> {
    events
        .into_iter()
        .map(|e| Captured {
            role: e.role,
            topic: e.topic,
            topic_expr: e.topic_expr,
            bus: e.bus_symbol,
            handler: e.handler,
            owner: e.owner,
        })
        .collect()
}

async fn events_for(filename: &str, src: &str) -> Vec<Captured> {
    let table = parse_src(filename, src).await;
    to_captured(table.events)
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_emit_and_on() {
    let src = r#"
function setup() {
  bus.emit("user.created", payload);
  bus.on("user.created", handleUser);
}
"#;
    let evs = events_for("ev.ts", src).await;
    assert_eq!(evs.len(), 2, "expected emit + on, got {evs:?}");
    assert!(matches!(evs[0].role, EventRole::Emit));
    assert_eq!(evs[0].topic, "user.created");
    assert_eq!(evs[0].bus.as_deref(), Some("bus"));
    assert!(matches!(evs[1].role, EventRole::Subscribe));
    assert_eq!(evs[1].handler.as_deref(), Some("handleUser"));
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_template_literal_topic_skipped() {
    let src = r#"
function setup() {
  bus.emit(`user.${id}.created`, payload);
}
"#;
    let evs = events_for("dyn.ts", src).await;
    assert!(evs.is_empty(), "template-substitution topics must be skipped, got {evs:?}");
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_identifier_topic_is_unresolved() {
    let src = r#"
function setup() {
  bus.on(eventName, handler);
}
"#;
    let evs = events_for("ident.ts", src).await;
    assert_eq!(evs.len(), 1);
    assert!(evs[0].topic.is_empty());
    assert_eq!(evs[0].topic_expr.as_deref(), Some("eventName"));
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_member_access_topic() {
    let src = r#"
function setup() {
  bus.on(EVENTS.USER_CREATED, handler);
}
"#;
    let evs = events_for("member.ts", src).await;
    assert_eq!(evs.len(), 1);
    assert!(evs[0].topic.is_empty());
    assert_eq!(evs[0].topic_expr.as_deref(), Some("EVENTS.USER_CREATED"));
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_const_string_folding() {
    let src = r#"
const TOPIC = "user.created";
const EVENTS = { USER_CREATED: "user.created", DELETED: "user.deleted" };
function setup() {
  bus.on(TOPIC, h1);
  bus.on(EVENTS.DELETED, h2);
}
"#;
    let table = parse_src("const.ts", src).await;
    let consts: std::collections::HashMap<_, _> = table.string_consts.iter().cloned().collect();
    assert_eq!(consts.get("TOPIC").map(String::as_str), Some("user.created"));
    assert_eq!(consts.get("EVENTS.USER_CREATED").map(String::as_str), Some("user.created"));
    assert_eq!(consts.get("EVENTS.DELETED").map(String::as_str), Some("user.deleted"));
    let evs = to_captured(table.events);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].topic_expr.as_deref(), Some("TOPIC"));
    assert_eq!(evs[1].topic_expr.as_deref(), Some("EVENTS.DELETED"));
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_inline_closure_falls_back_to_owner() {
    let src = r#"
function setup() {
  bus.on("ping", () => doThing());
}
"#;
    let evs = events_for("closure.ts", src).await;
    assert_eq!(evs.len(), 1);
    assert!(evs[0].handler.is_none());
    assert_eq!(evs[0].owner, "closure::setup");
}

#[cfg(feature = "lang-javascript")]
#[tokio::test]
async fn js_emitter_addlistener() {
    let src = r#"
function wire(em) {
  em.addListener('data', onData);
  em.publish('data', payload);
}
"#;
    let evs = events_for("ev.js", src).await;
    assert_eq!(evs.len(), 2);
    let subs: Vec<_> = evs.iter().filter(|e| matches!(e.role, EventRole::Subscribe)).collect();
    let pubs: Vec<_> = evs.iter().filter(|e| matches!(e.role, EventRole::Emit)).collect();
    assert_eq!(subs.len(), 1);
    assert_eq!(pubs.len(), 1);
}

#[cfg(feature = "lang-python")]
#[tokio::test]
async fn py_emit_and_subscribe() {
    let src = r#"
def setup(bus):
    bus.publish("user.created", payload)
    bus.subscribe("user.created", on_user)
"#;
    let evs = events_for("ev.py", src).await;
    assert_eq!(evs.len(), 2, "got {evs:?}");
    assert_eq!(evs[0].topic, "user.created");
    assert_eq!(evs[0].bus.as_deref(), Some("bus"));
    assert_eq!(evs[1].handler.as_deref(), Some("on_user"));
}

#[cfg(feature = "lang-python")]
#[tokio::test]
async fn py_skips_fstring_topic() {
    let src = r#"
def setup(bus, name):
    bus.publish(f"user.{name}.created", payload)
"#;
    let evs = events_for("fstr.py", src).await;
    assert!(evs.is_empty(), "f-string topics must be skipped, got {evs:?}");
}

#[cfg(feature = "lang-python")]
#[tokio::test]
async fn py_const_string_folding() {
    let src = r#"
TOPIC = "user.created"

def setup(bus):
    bus.subscribe(TOPIC, on_user)
"#;
    let table = parse_src("py_const.py", src).await;
    let consts: std::collections::HashMap<_, _> = table.string_consts.iter().cloned().collect();
    assert_eq!(consts.get("TOPIC").map(String::as_str), Some("user.created"));
    let evs = to_captured(table.events);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].topic_expr.as_deref(), Some("TOPIC"));
}

#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn rust_publish_and_subscribe() {
    let src = r#"
fn wire(tx: &Sender, bus: &Bus) {
    tx.publish("user.created");
    bus.subscribe("user.created", handler);
}
"#;
    let evs = events_for("ev.rs", src).await;
    assert_eq!(evs.len(), 2, "got {evs:?}");
    assert!(matches!(evs[0].role, EventRole::Emit));
    assert_eq!(evs[0].topic, "user.created");
    assert_eq!(evs[0].bus.as_deref(), Some("tx"));
    assert!(matches!(evs[1].role, EventRole::Subscribe));
    assert_eq!(evs[1].handler.as_deref(), Some("handler"));
}

#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn rust_identifier_topic_is_unresolved() {
    let src = r#"
fn wire(tx: &Sender) {
    tx.publish(payload);
}
"#;
    let evs = events_for("nonstr.rs", src).await;
    assert_eq!(evs.len(), 1);
    assert!(evs[0].topic.is_empty());
    assert_eq!(evs[0].topic_expr.as_deref(), Some("payload"));
}

#[cfg(feature = "lang-python")]
#[tokio::test]
async fn py_decorator_subscribe() {
    let src = r#"
@subscribe("user.created")
def on_user(payload):
    pass

@on_event("user.deleted")
async def on_user_deleted(payload):
    pass
"#;
    let evs = events_for("deco.py", src).await;
    assert_eq!(evs.len(), 2, "got {evs:?}");
    assert!(matches!(evs[0].role, EventRole::Subscribe));
    assert_eq!(evs[0].topic, "user.created");
    assert!(evs[0].handler.is_none());
    assert_eq!(evs[0].owner, "deco::on_user");
    assert_eq!(evs[1].topic, "user.deleted");
    assert_eq!(evs[1].owner, "deco::on_user_deleted");
}

#[cfg(feature = "lang-python")]
#[tokio::test]
async fn py_decorator_skips_non_whitelisted() {
    let src = r#"
@staticmethod
def helper():
    pass

@cache("user")
def cached():
    pass
"#;
    let evs = events_for("ndeco.py", src).await;
    assert!(evs.is_empty(), "non-whitelisted decorators must be skipped, got {evs:?}");
}

#[cfg(feature = "lang-typescript")]
#[tokio::test]
async fn ts_decorator_subscribe() {
    let src = r#"
class Handlers {
  @OnEvent('user.created')
  handleUser(payload: any) {}

  @EventPattern('user.deleted')
  handleDel(payload: any) {}
}
"#;
    let evs = events_for("deco.ts", src).await;
    assert_eq!(evs.len(), 2, "got {evs:?}");
    let topics: std::collections::HashSet<&str> =
        evs.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains("user.created"));
    assert!(topics.contains("user.deleted"));
    for e in &evs {
        assert!(matches!(e.role, EventRole::Subscribe));
        assert!(e.handler.is_none());
    }
}

#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn rust_const_string_folding() {
    let src = r#"
const TOPIC: &str = "user.created";

fn wire(tx: &Sender) {
    tx.publish(TOPIC);
}
"#;
    let table = parse_src("rs_const.rs", src).await;
    let consts: std::collections::HashMap<_, _> = table.string_consts.iter().cloned().collect();
    assert_eq!(consts.get("TOPIC").map(String::as_str), Some("user.created"));
    let evs = to_captured(table.events);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].topic_expr.as_deref(), Some("TOPIC"));
}
