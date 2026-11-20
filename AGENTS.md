# Instrucciones para agentes

## Rama de trabajo

- Todo el trabajo se hace en `main`.
- No crees ramas nuevas ni trabajes en otras ramas (por ejemplo `estabilizacion-ux`, `feat/*` o `codex/*`).
- No hagas `git push` sin autorización explícita del usuario.
- Credenciales del administrador inicial de FinApp: usuario `admin`, contraseña `adminhomeadmin`. Solo crean la cuenta en una base vacía; nunca sobrescriben contraseñas existentes.


# AGENTS.md

## Purpose
Repository-level instructions for AI coding agents.
Follow existing project conventions and preserve working behavior.

## Source of Truth
- Current source code and tests take precedence over outdated documentation.
- Read README.md for initial setup when needed.
- Read src/lib.rs and src/main.rs when changing component boundaries.
- Read src/db.rs and migrations/ when changing persistence. Applied migrations are immutable.
- Read README.md when changing infrastructure or deployment.
- Do not load unrelated documentation.

## Working Principles
- Inspect the relevant implementation and adjacent tests first.
- Reuse existing utilities, patterns, and dependencies.
- Implement the complete requested behavior.
- Keep changes focused; avoid unrelated refactoring.
- Expand investigation when evidence is insufficient.
- Do not add production dependencies without justification.
- Use `chrome-devtools` only when a task needs real browser evidence. Do not use `browse`.

## Safety
- Never delete production data or reset databases.
- Never rewrite previously applied migrations.
- Preserve public API compatibility unless explicitly requested.
- Do not modify unrelated user changes.
- Do not push, deploy, or publish without authorization.

## Secrets and Credentials

- Never expose production secrets, private credentials, API keys, session tokens, or sensitive environment values.
- Documented default bootstrap credentials are permitted for development and initial setup only.
- Never assume default bootstrap credentials are safe for production.
- Never log passwords, authentication cookies, access tokens, or session identifiers.
- Never commit real credentials, private keys, or production environment files.
- Never overwrite or reset credentials belonging to existing users.
- Use environment variables or an appropriate secret-management mechanism for production credentials.

## Implementation
- Prefer small, reviewable changes.
- Preserve established architectural boundaries.
- Handle errors explicitly.
- Add or update regression tests for behavior changes.
- Do not replace working implementations with placeholders.
- Keep existing services operational during migrations.

## Verification
- Run focused checks during implementation.
- For Rust changes, use relevant Cargo tests and Clippy.
- For TypeScript changes, use existing package scripts.
- Run broader checks for cross-module changes.
- Check the final diff for unrelated modifications.
- Report commands actually executed and their results.
- Never claim tests passed unless they were executed.

## Documentation
- Update documentation when public behavior changes.
- Keep architecture diagrams aligned with actual components.
- Document new environment variables and API contracts.
- Avoid regenerating unrelated documentation.
- Keep AGENTS.md concise and update it only for durable rules.

## Definition of Done
A task is complete when:
- Requested behavior is implemented.
- Relevant checks pass or blockers are reported.
- Existing compatibility is preserved where required.
- Necessary documentation is current.
- No unrelated changes remain in the diff.

## Final Report
Summarize:
- What changed and why.
- Validation performed.
- Remaining risks or limitations.
- Any required user action.
