//! Recall accuracy benchmark — inspired by LongMemEval / LoCoMo.
//!
//! Seeds a Thoth store with known facts, lessons, and code, then runs
//! a gold set of queries and asserts precision@k >= target.
//!
//! Run: `cargo test -p thoth-retrieve --test recall_accuracy -- --nocapture`

use tempfile::tempdir;
use thoth_core::{Enforcement, Fact, FactScope, Lesson, MemoryKind, MemoryMeta, Query};
use thoth_parse::LanguageRegistry;
use thoth_retrieve::{Indexer, Retriever};
use thoth_store::{MarkdownStore, StoreRoot};

struct GoldQuery {
    query: &'static str,
    expected_substring: &'static str,
}

fn gold_queries() -> Vec<GoldQuery> {
    vec![
        GoldQuery {
            query: "what database does the payment service use",
            expected_substring: "postgresql",
        },
        GoldQuery {
            query: "how to handle API rate limits",
            expected_substring: "backoff",
        },
        GoldQuery {
            query: "user authentication verify token",
            expected_substring: "verify",
        },
        GoldQuery {
            query: "deploy to production process",
            expected_substring: "staging",
        },
        GoldQuery {
            query: "when modifying cache",
            expected_substring: "invalidate",
        },
        GoldQuery {
            query: "retry policy for failed jobs",
            expected_substring: "retry",
        },
        GoldQuery {
            query: "application configuration loading",
            expected_substring: "load_config",
        },
        GoldQuery {
            query: "billing module ownership",
            expected_substring: "billing",
        },
        GoldQuery {
            query: "database migration best practices",
            expected_substring: "rollback",
        },
        GoldQuery {
            query: "HTTP error handling",
            expected_substring: "handle_error",
        },
    ]
}

fn make_fact(text: &str, tags: &[&str]) -> Fact {
    Fact {
        meta: MemoryMeta::new(MemoryKind::Semantic),
        text: text.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        scope: FactScope::default(),
    }
}

fn make_lesson(trigger: &str, advice: &str) -> Lesson {
    Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: trigger.to_string(),
        advice: advice.to_string(),
        success_count: 0,
        failure_count: 0,
        enforcement: Enforcement::default(),
        suggested_enforcement: None,
        block_message: None,
    }
}

fn seed_facts() -> Vec<Fact> {
    vec![
        make_fact(
            "The payment service uses PostgreSQL 15 with pgBouncer connection pooling.",
            &["infra", "payment"],
        ),
        make_fact(
            "Deploy to production: staging environment must pass all integration tests first. Run canary for 15 minutes.",
            &["deploy", "process"],
        ),
        make_fact(
            "Failed background jobs retry with exponential backoff, max 5 attempts, base delay 30s.",
            &["jobs", "retry"],
        ),
        make_fact(
            "The billing module owned by payments team (lead: @sarah). Slack: #billing-dev.",
            &["ownership", "billing"],
        ),
        make_fact(
            "Frontend uses React 18 with Next.js 14 app router. State via Zustand.",
            &["frontend", "tech-stack"],
        ),
        make_fact(
            "CI pipeline: GitHub Actions, runs on ubuntu-22.04, caches node_modules and .cargo.",
            &["ci", "infra"],
        ),
        make_fact(
            "Log aggregation via Datadog. Alerts fire to PagerDuty when p99 > 500ms.",
            &["observability", "alerts"],
        ),
        make_fact(
            "API versioning: URL prefix /v1/, /v2/. v1 deprecated but still in use by mobile app.",
            &["api", "versioning"],
        ),
    ]
}

fn seed_lessons() -> Vec<Lesson> {
    vec![
        make_lesson(
            "when handling API rate limits",
            "Use exponential backoff with jitter. Never retry immediately.",
        ),
        make_lesson(
            "when modifying the cache layer",
            "Always invalidate CDN after changing cache keys.",
        ),
        make_lesson(
            "when adding a new database migration",
            "Always add a rollback script. Test on a copy of production data first.",
        ),
        make_lesson(
            "when refactoring shared utilities",
            "Check all downstream consumers first.",
        ),
        make_lesson(
            "when updating environment variables",
            "Update both .env.example and deployment docs.",
        ),
    ]
}

const CODE_AUTH: &str = r#"
pub mod auth {
    pub struct Claims {
        pub sub: String,
        pub exp: u64,
    }

    /// Verifies a JWT token and returns the decoded claims.
    pub fn verify_jwt(token: &str) -> Result<Claims, String> {
        if token.is_empty() {
            return Err("missing token".into());
        }
        Ok(Claims { sub: "user".into(), exp: 0 })
    }
}
"#;

const CODE_CONFIG: &str = r#"
pub mod config {
    use std::path::Path;

    pub struct AppConfig {
        pub database_url: String,
        pub port: u16,
    }

    /// Loads configuration from TOML file, then overlays environment variables.
    pub fn load_config(path: &Path) -> Result<AppConfig, String> {
        Ok(AppConfig { database_url: String::new(), port: 8080 })
    }
}
"#;

const CODE_ERROR: &str = r#"
pub mod errors {
    /// Central error handler for the HTTP layer.
    pub fn handle_error(status: u16, message: &str) -> String {
        format!("{status}: {message}")
    }
}
"#;

#[tokio::test]
async fn recall_accuracy_at_k5() {
    let root_dir = tempdir().unwrap();
    let src_dir = tempdir().unwrap();

    for (name, content) in [
        ("auth.rs", CODE_AUTH),
        ("config.rs", CODE_CONFIG),
        ("errors.rs", CODE_ERROR),
    ] {
        tokio::fs::write(src_dir.path().join(name), content)
            .await
            .unwrap();
    }

    let store = StoreRoot::open(root_dir.path()).await.unwrap();
    let indexer = Indexer::new(store.clone(), LanguageRegistry::new());
    indexer.index_path(src_dir.path()).await.unwrap();

    let md_store = MarkdownStore::open(root_dir.path()).await.unwrap();
    for fact in seed_facts() {
        md_store.append_fact(&fact).await.unwrap();
    }
    for lesson in seed_lessons() {
        md_store.append_lesson(&lesson).await.unwrap();
    }

    let retriever = Retriever::new(store);
    let queries = gold_queries();
    let total = queries.len();
    let mut hits = 0usize;

    for gq in &queries {
        let q = Query {
            text: gq.query.to_string(),
            top_k: 5,
            ..Query::text("")
        };
        let out = retriever.recall(&q).await.unwrap();

        let found = out.chunks.iter().any(|c| {
            let text = format!("{} {}", c.preview, c.body).to_lowercase();
            text.contains(gq.expected_substring)
        });

        if found {
            hits += 1;
        } else {
            eprintln!(
                "MISS: query={:?} expected={:?}",
                gq.query, gq.expected_substring,
            );
            for (i, c) in out.chunks.iter().enumerate() {
                eprintln!(
                    "  [{i}] score={:.3} path={} preview={:.80}",
                    c.score,
                    c.path.display(),
                    c.preview
                );
            }
        }
    }

    let precision = (hits as f64) / (total as f64) * 100.0;
    eprintln!("\n=== Recall Accuracy @k=5 ===");
    eprintln!("Hits: {hits}/{total} = {precision:.1}%");

    assert!(
        precision >= 80.0,
        "recall precision@5 {precision:.1}% below 80% target ({hits}/{total})"
    );
}

#[tokio::test]
async fn recall_accuracy_at_k3() {
    let root_dir = tempdir().unwrap();
    let src_dir = tempdir().unwrap();

    for (name, content) in [
        ("auth.rs", CODE_AUTH),
        ("config.rs", CODE_CONFIG),
        ("errors.rs", CODE_ERROR),
    ] {
        tokio::fs::write(src_dir.path().join(name), content)
            .await
            .unwrap();
    }

    let store = StoreRoot::open(root_dir.path()).await.unwrap();
    let indexer = Indexer::new(store.clone(), LanguageRegistry::new());
    indexer.index_path(src_dir.path()).await.unwrap();

    let md_store = MarkdownStore::open(root_dir.path()).await.unwrap();
    for fact in seed_facts() {
        md_store.append_fact(&fact).await.unwrap();
    }
    for lesson in seed_lessons() {
        md_store.append_lesson(&lesson).await.unwrap();
    }

    let retriever = Retriever::new(store);
    let queries = gold_queries();
    let total = queries.len();
    let mut hits = 0usize;

    for gq in &queries {
        let q = Query {
            text: gq.query.to_string(),
            top_k: 3,
            ..Query::text("")
        };
        let out = retriever.recall(&q).await.unwrap();

        let found = out.chunks.iter().any(|c| {
            let text = format!("{} {}", c.preview, c.body).to_lowercase();
            text.contains(gq.expected_substring)
        });

        if found {
            hits += 1;
        }
    }

    let precision = (hits as f64) / (total as f64) * 100.0;
    eprintln!("\n=== Recall Accuracy @k=3 ===");
    eprintln!("Hits: {hits}/{total} = {precision:.1}%");

    assert!(
        precision >= 60.0,
        "recall precision@3 {precision:.1}% below 60% target ({hits}/{total})"
    );
}
