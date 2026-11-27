# PM checks — the single source of truth. Every check is defined exactly once
# here; pre-commit (the fast subset) and the CI workflows (.github/workflows/)
# invoke these same recipes, so local and CI can never drift.
#
#   just            # list recipes
#   just check-fast # the fast subset (what pre-commit runs)
#   just check      # everything (what a full PR run covers)
#   just fmt        # auto-apply every formatter
#
# Recipes run under bash on every OS (Linux CI + the maintainer's Git-Bash), so
# there are no PowerShell-isms to leak onto the Linux runners.
set shell := ["bash", "-c"]

manifest := "src-tauri/Cargo.toml"
# The sealed-frame regression app (Mods spec §1.4): its own crate and lock, never built into PM. Only
# formatting is gated here; .github/workflows/sealed-frame.yml builds and runs it on three OSes.
sealed_frame := "tests/sealed-frame/src-tauri/Cargo.toml"

# Default: show the recipe list.
default:
    @just --list

# --- aggregates -----------------------------------------------------------

# The fast subset (formatting, types, lint, bespoke gates) — what pre-commit runs.
# `just ci-membership` asserts BOTH directions of that claim: every member below has a
# step in pr.yml AND a hook in .pre-commit-config.yaml. The claim used to be prose only,
# and pre-commit had drifted to 9 of the 13 — missing, of all things, the drift guards.
check-fast: prettier eslint tsc cargo-fmt ruff ruff-fmt version files headers mods license-subset ci-membership sync-set ipc-commands calendar-write-fence tasks-emit people-emit script-deps provider-checks-offline action-pins requirements-lock node-version npm-licenses sidecar-licences model-licences

# Everything a PR is gated on (adds the compile/test/supply-chain/security checks).
check: check-fast frontend-test build-frontend build-mods clippy cargo-check rust-test sidecar-test deny pip-audit npm-audit gitleaks gitleaks-history zizmor

# Auto-apply every formatter (the writing counterpart to the --check recipes).
fmt:
    npx prettier --write .
    npx eslint . --fix || true
    cargo fmt --manifest-path {{manifest}}
    cargo fmt --manifest-path {{sealed_frame}}
    ruff check --fix sidecar
    ruff format sidecar

# --- frontend (TS / React) ------------------------------------------------

prettier:
    npx prettier --check .

eslint:
    npx eslint .

# Type-check every TS project: the app (src), the Node-side config (vite.config.ts,
# via the tsconfig.node.json project reference) — plain `tsc` only checks the root
# project, so vite.config.ts went unchecked (T1-9) — and the mods. Separate `-p` passes, not
# `tsc -b`: build mode requires composite refs to emit, which --noEmit forbids (TS6310).
# The mods get two passes because a Worker has a different global scope (lib WebWorker, no DOM) and
# the two libs cannot share a config: `mods` is the page, `mods/atlas.map/src/worker` the Map's Worker
# (each further mod worker folder adds a line here and an `exclude` in mods/tsconfig.json).
tsc:
    npx tsc --noEmit
    npx tsc -p tsconfig.node.json --noEmit
    npx tsc -p mods --noEmit
    npx tsc -p mods/atlas.map/src/worker --noEmit

# Frontend unit tests (vitest). Started as T-07 over the pure src/lib/** modules the audit flagged as
# guarded by nothing (date formatting, the markdown sanitize allowlist), and now also covers component
# tests and scripts/**/*.test.mjs — so "frontend" undersells it; see vitest.config.ts for the globs.
frontend-test:
    npx vitest run

# Build the PRODUCTION webview bundle — the artifact `tauri build` actually ships.
# `tsc --noEmit`, ESLint and vitest between them never run the bundler, so a broken
# dynamic import, a missing asset, a Tailwind/plugin config error or a dependency that
# simply won't bundle was invisible to every PR and first appeared inside the release
# job. In `check` rather than `check-fast`: it is the one frontend check slow enough
# to notice on every commit.
build-frontend:
    npx vite build

# Build, stage, pack and verify every mod under mods/ (Mods spec §7). For each folder with a
# manifest: Vite builds it into dist/app (mods/vite.config.ts), `pack-mod --stage` puts manifest,
# licence and notice beside it (dist/ is then a folder the developer-mode loader opens), `pack-mod`
# writes dist/<id>-<version>.pmmod stamped with this checkout's commit, and `pack-mod --verify` reads
# that file back with the independent check (an author's lint, the frame's CSP is the guard: no
# inline script or style, no module script, no `crossorigin`, which the hygiene job cannot do: it
# has no build). The file is NOT signed: Atlas's
# only signer is the pm-mods pipeline. Fails when there is no mod to build, so a moved folder cannot
# leave this a green step that built nothing.
build-mods:
    #!/usr/bin/env bash
    set -euo pipefail
    commit="$(git rev-parse HEAD)"
    built=0
    for dir in mods/*/; do
        dir="${dir%/}"
        [ -f "$dir/manifest.json" ] || continue
        PM_MOD_DIR="$dir" npx vite build --config mods/vite.config.ts
        node scripts/pack-mod.mjs "$dir" --stage --commit "$commit"
        rm -f "$dir"/dist/*.pmmod
        node scripts/pack-mod.mjs "$dir" --commit "$commit"
        node scripts/pack-mod.mjs --verify "$dir"/dist/*.pmmod
        built=$((built + 1))
    done
    if [ "$built" -eq 0 ]; then
        echo "✗ build-mods: no mod folder (a manifest.json) under mods/" >&2
        exit 1
    fi
    echo "✓ build-mods: built, packed and verified $built mod(s)"

# Pack the acme.hello test fixture as a developer's own file (`--unsigned`, tier third), which
# `build-mods` skips because it only loops over mods/. This is the file the L7 live test installs
# from a debug PM's developer-mode "install from file". Writes
# tests/mod-fixtures/acme.hello/dist/acme.hello-<version>.pmmod (git-ignored).
build-fixture:
    PM_MOD_DIR=tests/mod-fixtures/acme.hello npx vite build --config mods/vite.config.ts
    node scripts/pack-mod.mjs tests/mod-fixtures/acme.hello --unsigned
    node scripts/pack-mod.mjs --verify tests/mod-fixtures/acme.hello/dist/*.pmmod

# --- backend (Rust) -------------------------------------------------------

# Fetch the bundled standalone interpreter before anything that compiles the
# crate. On Windows and Linux the merged tauri.<platform>.conf.json bundles
# `src-tauri/python/` as a resource, so the Tauri build script needs it present;
# this fetches it (idempotent — the .pm-pyver stamp skips when current, so it's
# a one-time ~70 MB download per machine; CI runners re-download each run since
# nothing caches src-tauri/python). A NO-OP on macOS, which downloads its
# interpreter at runtime instead (python_fetch.rs).
fetch-python:
    node scripts/fetch-python.mjs

# Refresh the curated local-model catalog (#296) from Hugging Face into
# src-tauri/local_models.json. Dev-only + network-bound, so deliberately NOT part of
# `check` (it must never make the PR gate flaky). Uses the @huggingface/gguf
# devDependency; `--discover` prints the top GGUF repos as a curation aid. Idempotent —
# rewrites only on a real content change.
#
# `local_models.json` is `.prettierignore`d (the generator owns its format), but the licence ledger
# it writes alongside is NOT — and `JSON.stringify(…, null, 2)` never collapses a short array the
# way Prettier does. So even a run that changed nothing left `model_licences.json` dirty and failed
# `just prettier`, which reads as a broken gate rather than the expected output of a generator. Same
# trap as `lock-regen`; formatted here for the same reason. This matters most for #447, where a
# scheduled Action opens the PR and nobody is watching to re-run the formatter by hand.
generate-local-catalog:
    node scripts/generate-local-catalog.mjs
    npx prettier --write src-tauri/model_licences.json

cargo-fmt:
    cargo fmt --check --manifest-path {{manifest}}
    cargo fmt --check --manifest-path {{sealed_frame}}

clippy: fetch-python
    cargo clippy --all-targets --all-features --manifest-path {{manifest}} -- -D warnings

cargo-check: fetch-python
    cargo check --all-targets --tests --manifest-path {{manifest}}

rust-test: fetch-python
    cargo test --manifest-path {{manifest}}

# Local-only test coverage for spotting blind spots (needs `cargo install cargo-llvm-cov`).
# Deliberately NOT in `check` and never gated on a percentage — visibility, not a bar (T1-11).
coverage: fetch-python
    cargo llvm-cov --manifest-path {{manifest}}

# --- sidecar (Python) -----------------------------------------------------

ruff:
    ruff check sidecar

ruff-fmt:
    ruff format --check sidecar

# Fast sidecar unit tests (standard library only; the real-tokenizer check
# self-skips when fastembed isn't installed, e.g. on CI). Locks the token-count
# padding fix — see sidecar/test_pm_sidecar.py.
sidecar-test:
    python sidecar/test_pm_sidecar.py

# --- supply chain & secrets ----------------------------------------------

# Advisories + licences + bans + sources in one (config: src-tauri/deny.toml).
deny:
    cd src-tauri && cargo deny check

# Python dependency CVE audit. Scans the LOCKS — which is every package that actually installs,
# transitive ones included, on the base venv AND every optional component (L-6). It used to scan
# requirements.txt, i.e. the six top-level pins only, so a CVE in (say) pdfminer.six or lxml — both
# of which parse untrusted input — was never looked at.
#
# `--disable-pip`: audit the pinned versions as written instead of pip-installing the locks into a
# throwaway venv first. pip-audit allows it only for hashed requirements, which every lock line is,
# and it does not resolve anything, so it audits exactly the lock. Installing them was also how the
# audit broke: an override in sidecar/requirements-override.txt (fastembed's `pillow<13`, until
# fastembed 0.9.1) is a lock pip's resolver refuses. Like the install it replaces, it reads the lines whose markers
# match the auditing interpreter (3.12 in CI).
pip-audit:
    pip-audit --disable-pip -r sidecar/requirements.lock -r sidecar/requirements-ocr.lock

# JS dependency CVE audit against the npm lockfile. `moderate` (not `high`) so a moderate-rated
# sanitizer / proto-pollution advisory in the render path can't pass green (L-7). The script runs
# `npm audit` and lets through only an advisory listed in scripts/npm-audit-rules.mjs's ACCEPTED (dated,
# with the decision),
# and only while a second audit of the shipped tree alone (`--omit=dev`) doesn't report it.
npm-audit:
    node scripts/npm-audit.mjs

# Secret scan of the WHOLE working tree — including git-ignored files, so it's the
# sole net for a secret in an ignored path (config: .gitleaks.toml). CI runs this
# same recipe over its checkout — it does NOT scan a diff (T1-12).
gitleaks:
    gitleaks dir . --config .gitleaks.toml --redact --no-banner

# Secret scan of git HISTORY (every commit) — `dir` mode above misses a secret that
# was committed and later deleted, which lingers in history on a public repo. Fast
# (the whole history is small); CI needs a full-depth checkout to see every commit (T1-12).
gitleaks-history:
    gitleaks git . --config .gitleaks.toml --redact --no-banner

# Workflow-security audit (the pull_request_target / fork-secret class).
zizmor:
    zizmor .github/workflows

# --- bespoke gates (pure Node, no deps) -----------------------------------

# Version lockstep + changelog; pass --base <ref> to require a bump, --tag <vX.Y.Z> to match a tag.
# With --base, a change that touches only mod folders (mods/<id>/) is exempt from PM's bump and owes its mod's own (D12).
version *args:
    node scripts/check-version-lockstep.mjs {{args}}

# Pre-commit's version check: try the bumped-vs-base check against origin/main so a
# forgotten bump is caught locally (not just in CI), but fall back to lockstep-only
# with a warning when the ref isn't fetched — an offline commit isn't blocked (T1-10).
version-local:
    @if git rev-parse --verify --quiet origin/main >/dev/null 2>&1; then \
        node scripts/check-version-lockstep.mjs --base origin/main; \
    else \
        echo "version-local: origin/main not fetched — lockstep only (bump-vs-base deferred to CI)"; \
        node scripts/check-version-lockstep.mjs; \
    fi

files:
    node scripts/check-files-in-place.mjs

headers:
    node scripts/check-spdx-headers.mjs

# The mods folder (Mods spec §7, I-25), from source alone: each manifest passes package.rs's rules,
# id = folder, atlas.* and first party, a changelog that matches, nothing under mods/ in any tauri
# bundle's resources, nothing in src/ importing mods/. It never reads built output: this runs in
# pr.yml's hygiene job, which has no build (`pack-mod --verify`, inside build-mods, does that).
# `--base <ref>` adds "a changed mod bumped its version"; `just version --base` runs that same check.
mods *args:
    node scripts/check-mods.mjs {{args}}

# Licence allow-lists agree: everything deny.toml permits (PR gate) is attributable
# in about.toml's accepted set (release NOTICE), so NOTICE generation can't hit an
# unexpected licence after signing (T-02).
license-subset:
    node scripts/check-license-subset.mjs

# Every `just check` recipe is wired into pr.yml, so a gate added here can't silently
# skip CI until pr.yml is separately edited (T-04).
ci-membership:
    node scripts/check-ci-membership.mjs

# Every table the schema creates is classified in SYNC-SET.md (truth / derived / device /
# mixed), so a new table can't land without a declared owner of truth for a future sync.
sync-set:
    node scripts/check-sync-set.mjs

# Every command name src/lib/ipc.ts invokes is one the backend registers, and every registered
# command is reachable from a wrapper. The names are bare string literals on both sides, so nothing
# else compares them: a renamed Rust command surfaces as a runtime "command not found" the first
# time that particular screen is opened, which can be a release later.
ipc-commands:
    node scripts/check-ipc-commands.mjs

# Calendar writes (#884) stay where they were put: the commands, the private Google sender and the
# write core are named only by the editing module, its registration and the editor's hook, and no
# module on a model path names any of them or sends an authorised Google request.
calendar-write-fence:
    node scripts/check-calendar-write-fence.mjs

# "The task list changed" (`tasks://changed`) is announced from one place, `tasks::announce_changed`:
# the constant is named nowhere else in the Rust code, so a new writer (a mod's handler, a command)
# can't emit it a different way, or forget to, or send it while the DB guard is held.
tasks-emit:
    node scripts/check-tasks-emit.mjs

# "The address book changed" (`people://changed`) is announced from one place,
# `people::announce_changed`, for the same reason: no command, import, merge or sync pass can emit
# it a different way, or forget to, or send it while the DB guard is held.
people-emit:
    node scripts/check-people-emit.mjs

# scripts/ stays zero-dependency (INVARIANTS.md I-18): node: builtins and repo-relative paths only,
# plus a small allowlist of justified exceptions inside the check itself. Not taste — pr.yml's
# hygiene job runs no `npm ci`, so an unlisted import in a gate script passes locally and dies only
# in CI. Also asserts each exception is still imported, dev-only, and exactly pinned.
script-deps:
    node scripts/check-script-deps.mjs

# The weekly provider canary's offline suite (scripts/provider-checks, node:test, no dependencies, so it
# runs in pr.yml's hygiene job with no `npm ci`): recorded vendor replies and deliberately drifted copies
# through a fake fetch and a fake `gh`, the usage-key allow-list, the golden request bodies, the
# no-print scan and the workflow's security shape. Node 24's runner takes a glob, not a folder. The live
# canary (.github/workflows/provider-canary.yml) needs keys and the network, so it is not a member of
# `just check`; this is. vitest skips this folder (vitest.config.ts) because these files run here.
provider-checks-offline:
    node --test 'scripts/provider-checks/**/*.test.mjs'

# Every `uses:` in .github/workflows is pinned to a 40-char commit SHA with a readable
# version comment. Both workflow headers already CLAIM "the repo enforces it" — until
# now nothing did, so a `uses: foo/bar@v3` would have sailed through while the repo
# kept asserting the opposite.
action-pins:
    node scripts/check-action-pins.mjs

# The sidecar's dependency locks are current, fully pinned and fully hashed. Offline and
# zero-dependency: each lock stamps the SHA-256 of every input it was generated from, and this
# recomputes them. Also in release.yml's `guards` job — a stale lock blocks a release, not just a
# merge, because the lock is what installs on a user's machine.
requirements-lock:
    node scripts/check-requirements-lock.mjs

# Every CI job runs the Node major package.json declares. The version used to live only in eleven
# hand-maintained workflow pins, so local and CI drifted apart unnoticed — PM was developed on 24
# while CI built and bundled on 20, months after 20 went end-of-life.
node-version:
    node scripts/check-node-version.mjs

# Every npm package that SHIPS carries an accepted licence. The npm half of what cargo-deny does
# for crates: pr.yml's `dependencies` job was named for all three ecosystems and gated only Rust, so
# a copyleft package arriving transitively — through a patch bump four levels down — would first
# have been noticed as a complaint about a released binary. Offline: package-lock.json records a
# licence for every package and marks the dev-only ones.
npm-licenses:
    node scripts/check-npm-licenses.mjs

# Every Python package the sidecar installs is under a licence someone read and accepted — the
# third ecosystem in the job named for all three. Compares sidecar/licences.json against the locks:
# every locked package covered, at every locked version, by a value written BY HAND. PyPI's own
# metadata is too unreliable to normalise automatically (pillow-heif declared BSD-3-Clause while
# classifying itself GPLv2), so the network and the review live in `just lock-regen`; this is
# offline and reaches nothing.
sidecar-licences:
    node scripts/check-sidecar-licences.mjs

# Every model in the curated local-AI catalogue is under a licence someone read, and the catalogue's
# copy of it still matches the ledger it came from. Seven of the seventeen are under bespoke
# publisher terms rather than an open-source licence (Gemma, Llama, the largest Qwen 2.5) and the
# catalogue carried nothing about any of it. Offline: two committed JSON files and one Rust file.
model-licences:
    node scripts/check-model-licences.mjs

# --- generators (not part of `check`) -------------------------------------

# Regenerate the sidecar dependency locks and refresh the licence evidence behind them. Needs `uv`
# on PATH; both halves reach the network. Run after any change to sidecar/requirements.txt or the
# optional pins in sidecar.rs, review anything the second half flags, then commit the result.
# Every run also takes whatever upstream released since the last one; `PM_LOCK_KEEP=1 just
# lock-regen` keeps the versions the committed locks hold unless an input forces a move, for a
# change that only moves packages between locks (scripts/regen-sidecar-locks.mjs says more).
# `sidecar/licences.json` is one of the few generated files Prettier still owns, and
# `JSON.stringify(…, null, 2)` never collapses a short array the way Prettier does — so the
# generator's own output failed `just prettier` every single time it ran. Format it here rather
# than leaving every lock-regen to end in a formatting failure with no hint that it was expected.
lock-regen:
    node scripts/regen-sidecar-locks.mjs
    node scripts/regen-sidecar-licences.mjs
    npx prettier --write sidecar/licences.json

# --- release-only ---------------------------------------------------------

# Regenerate the third-party licence NOTICE: the bundled Rust crates, then the npm packages
# compiled into the shipped webview bundle. The second half used to be missing entirely, so 122
# production packages — the four self-hosted @fontsource families among them — were conveyed with no
# attribution, though MIT, ISC and OFL-1.1 all require their notices to travel with the software.
# Needs `cargo about` AND node_modules (`npm ci`); the npm generator fails rather than emitting a
# short NOTICE, because a partial attribution file reads as compliance.
notice out="THIRD-PARTY-NOTICES.txt":
    just notice-rust "{{out}}"
    node scripts/generate-npm-notice.mjs >> "{{out}}"

# The Rust half on its own. pr.yml's `dependencies` job renders it on every PR, so a fault surfaces
# there rather than in the release's publish job. It fails on two faults cargo-about lets through
# with exit 0: a clarification in src-tauri/about.toml whose checksum stopped matching, which only
# warns and then drops that crate's licence text, and a template field cargo-about doesn't provide,
# which renders as nothing. The second shipped every release from #36 to 3.164.1 with crate names
# and no licence texts, because about.hbs asked for `license_text`.
notice-rust out:
    @log="$(mktemp)"; \
    cargo about generate -m {{manifest}} src-tauri/about.hbs -o "{{out}}" 2>"$log"; status=$?; \
    cat "$log" >&2; \
    if [ "$status" -ne 0 ]; then rm -f "$log"; exit "$status"; fi; \
    if grep -q "failed to validate" "$log"; then \
        rm -f "$log"; \
        echo "✗ NOTICE: a clarification in src-tauri/about.toml no longer matches its crate (warning above). Read the crate's new licence files, then update the checksums." >&2; \
        exit 1; \
    fi; \
    rm -f "$log"; \
    if ! grep -q "Permission is hereby granted" "{{out}}"; then \
        echo "✗ NOTICE: no licence text rendered into {{out}}. Check the field names in src-tauri/about.hbs." >&2; \
        exit 1; \
    fi
