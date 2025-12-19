# Copilot PR Review Instructions

## Scope
- Only comment on issues that are **specific to the diff** (avoid generic best-practice reminders).
- Avoid repeating the same point across multiple files. If one example demonstrates a pattern, mention it once and reference the pattern.

## What to focus on (in priority order)
1) Correctness and logic bugs
2) Security issues (input validation, authz/authn, secrets, injection)
3) Performance regressions (hot paths only)
4) API/contract changes and backward compatibility
5) Test gaps only when risk is high or behavior changed

## What to avoid
- Do NOT suggest stylistic refactors unless they fix a bug or prevent a clear maintenance issue.
- Do NOT request documentation unless public APIs changed.
- Do NOT comment on naming unless it causes real ambiguity.
- Do NOT suggest "add null checks" if the code is already guarded or types guarantee non-null.

## Output style
- Prefer fewer, higher-signal comments.
- Use this structure when leaving feedback:
  - **Issue** (why it matters)
  - **Evidence** (where in diff / what behavior)
  - **Suggestion** (concrete fix)
