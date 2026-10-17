mod common;

use std::time::{Duration, Instant};

use common::{TestRepo, pattern_clause, protected_clause};
use serde_json::json;

const BUDGET: Duration = Duration::from_millis(20);

/// A TypeScript file of about 2,500 lines with imports and some existing violations, so a
/// violating edit makes the check parse both versions: the worst case.
fn large_file() -> String {
    let mut source = String::new();
    for i in 0..500 {
        source.push_str(&format!("import {{ dep{i} }} from './deps/dep{i}';\n"));
        let body = if i % 50 == 0 {
            "asProvisioned(t)!.id"
        } else {
            "asProvisioned(t)?.id"
        };
        source.push_str(&format!("export function task{i}(t: Task) {{\n  const id = {body};\n  return id ?? dep{i};\n}}\n"));
    }
    source
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() * 95 / 100 - 1]
}

/// Spec section 7: an edit check takes under 20 ms. Timing depends on the build, so this runs
/// only in release mode, through the Nx `bench` target:
/// `cargo test --release -p pragma-cli --test latency -- --ignored --nocapture`
#[test]
#[ignore = "timing test; run in release mode with the bench target"]
fn edit_check_p95_is_under_budget_on_a_large_file() {
    let repo = TestRepo::new();
    repo.lock(json!([
        pattern_clause("bang", "asProvisioned($$$A)!", &[], "asProvisioned(t)!", "asProvisioned(t)"),
        pattern_clause("project-bang", "asAvailableProject($$$A)!", &[], "asAvailableProject(p)!", "asAvailableProject(p)"),
        pattern_clause("require", "require($$$A)", &["**/*.ts"], "require('a')", "import a from 'a'"),
        pattern_clause("task-manager", "$OBJ.taskManager", &[], "project.taskManager", "getTaskManagerStore(id)"),
        protected_clause("migrations", &["drizzle/[0-9]*.sql"]),
        {
            "id": "boundary",
            "anchor": common::anchor(30),
            "text": "Main must not import the renderer.",
            "scope": ["src/**"],
            "check": { "kind": "library", "rule": "import-boundary", "forbid": ["@renderer/**"] },
            "fix": "Go through Wire."
        }
    ]));
    repo.write("src/big.ts", &large_file());
    let violating = |run: usize| {
        let mut input = repo.edit_input(
            "src/big.ts",
            "export function task250(",
            "const extra = asProvisioned(t)!.id;\nexport function task250(",
        );
        input["session_id"] = json!(format!("bench-{run}"));
        input
    };
    let clean = repo.edit_input(
        "src/big.ts",
        "export function task250(",
        "export function renamedTask250(",
    );
    for run in 0..3 {
        repo.hook(violating(run), &[]);
    }
    let time = |input: serde_json::Value, expected: i32| {
        let started = Instant::now();
        let output = repo.hook(input, &[]);
        let elapsed = started.elapsed();
        assert_eq!(output.status.code(), Some(expected));
        elapsed
    };
    let worst: Vec<Duration> = (10..50).map(|run| time(violating(run), 2)).collect();
    let typical: Vec<Duration> = (0..40).map(|_| time(clean.clone(), 0)).collect();
    // A clean edit in a session with an open block: the allowed edit that also writes.
    let clearing: Vec<Duration> = (50..90)
        .map(|run| {
            let setup = repo.hook(violating(run), &[]);
            assert_eq!(
                setup.status.code(),
                Some(2),
                "the setup edit must block, or there is no open block to clear"
            );
            let mut input = clean.clone();
            input["session_id"] = json!(format!("bench-{run}"));
            time(input, 0)
        })
        .collect();
    let (worst, typical, clearing) = (p95(worst), p95(typical), p95(clearing));
    println!(
        "edit check p95: {worst:?} with a violation, {typical:?} without, {clearing:?} clearing a block"
    );
    assert!(
        worst < BUDGET,
        "p95 {worst:?} with a violation is over {BUDGET:?}"
    );
    assert!(
        typical < BUDGET,
        "p95 {typical:?} without a violation is over {BUDGET:?}"
    );
    assert!(
        clearing < BUDGET,
        "p95 {clearing:?} clearing a block is over {BUDGET:?}"
    );
}
