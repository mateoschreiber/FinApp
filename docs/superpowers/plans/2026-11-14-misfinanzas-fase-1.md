# MisFinanzas Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the approved local-first, multi-user personal finance PWA.

**Architecture:** Rust HTTP service and SQLite own authentication, authorization, and the authoritative financial ledger. A lightweight browser client maintains a durable per-user offline copy and synchronizes versioned operations with the server.

**Tech Stack:** Rust, SQLite, Docker Compose, HTML/CSS/JavaScript, IndexedDB, service worker.

## Global Constraints

- Follow `docs/superpowers/specs/2026-11-14-misfinanzas-fase-1-design.md`; PYG values are integer, language Spanish, timezone America/Asuncion.
- Do not publish the server to the internet. No embedded secrets, open registration, or token storage in JavaScript-accessible storage.
- Write a failing behavioral test before each implementation step; demonstrate the failure and then make it pass. Never mark a task complete without its verification.

## Tasks

### 1. Server foundation and access

**Files:** `Cargo.toml`, `src/main.rs`, `src/db.rs`, `src/auth.rs`, `tests/auth.rs`, `migrations/`.

- [ ] Create HTTP service, migration-backed SQLite database, initial administrator bootstrap, secure password hashes and server sessions. Authenticate via HTTP-only same-site cookies; use server-side admin authorization and per-user ownership checks.
- [ ] Test invalid credentials, session expiry, administrator-only user creation and cross-user isolation with an actual temporary database.
- [ ] Verify `cargo test` and record the result before advancing.

### 2. Financial ledger and synchronization API

**Files:** `src/ledger.rs`, `src/sync.rs`, `tests/ledger.rs`, `tests/sync.rs`, `migrations/`.

- [ ] Add account and category management and versioned income, expense and card-payment operations. Derive balances using signed integer arithmetic; apply payment effects atomically.
- [ ] Give each client operation a stable idempotency key; reject stale versions as conflicts, preserve the authoritative version and return it to the client. Scope all records and mutations to the authenticated user.
- [ ] Test arithmetic, editing and deleting payments, account and category validation, duplicate retries, conflict cases and cross-user requests.

### 3. Installable offline client

**Files:** `static/index.html`, `static/app.js`, `static/db.js`, `static/sync.js`, `static/sw.js`, `static/manifest.webmanifest`, `static/style.css`, browser tests.

- [ ] Implement Spanish authentication, administrator user creation, account/category management, ledger and balance views, plus movement and card-payment forms.
- [ ] Persist per-user data and queued operations in IndexedDB. On reconnection synchronize while open, display pending changes and resolve conflicts manually without losing either version. Warn before discarding pending changes at logout.
- [ ] Cache the application shell with a versioned service worker; test offline reload, creation/edit/deletion and subsequent synchronization in a real browser.

### 4. Deployment, recovery and release validation

**Files:** `Dockerfile`, `compose.yaml`, `.dockerignore`, `README.md`, deployment and backup documentation.

- [ ] Add reproducible container build, persistent SQLite volume, health check and reverse-proxy configuration guidance for local HTTPS with Cloudflare DNS validation. Do not commit credentials.
- [ ] Document initial admin provisioning, operation, SQLite-consistent manual backup and restoration, and local-device data limitations.
- [ ] Run formatting, lint, Rust tests, client tests, dependency audit, container build, restore rehearsal and Android/desktop browser scenarios; explicitly report any environment-dependent verification that cannot run.
