---
name: squall-latest
description: "Pull the latest model catalogs (Together, Codex, Claude, Antigravity, xAI, OpenRouter), probe every configured model with a live dispatch, and update Squall's roster, aliases, tests, README and skill prompts. Use when asked to 'squall latest', 'pull latest models', 'refresh the roster', 'update models', 'check for new models', or when a review fails because a model is dead. Also runs unattended from the weekly launchd job. (project)"
one_liner: "Catalogs lie and rosters rot; probe every model, then rename through the alias table so nothing callers depend on breaks."
activation_triggers:
  - "squall latest"
  - "/squall-latest"
  - "pull latest models"
  - "refresh model roster"
  - "update models"
  - "check for new models"
  - When a Squall dispatch fails with a non-serverless / unknown-model error
  - When the weekly launchd job (scripts/squall-latest-weekly.sh) invokes it
related_skills:
  - "[squall-unified-review](../squall-unified-review/SKILL.md) (consumes the model keys this skill renames)"
  - "[squall-research](../squall-research/SKILL.md) (also names model keys)"
---

# Squall Latest

> **TL;DR - QUICK REFERENCE**
>
> | Step | Command / file |
> |------|----------------|
> | Audit (facts only) | `python3 -I scripts/model-audit.py` → `.squall/model-audit/<date>.md` |
> | Exit 0 / 10 / 20 | healthy, nothing new / healthy, new entries / a configured model is DEAD |
> | Roster | `BUILTIN_DEFAULTS` in `src/config.rs` |
> | Renames | `RETIRED_MODEL_ALIASES` (old key AND old model_id → new key) |
> | No successor | `REMOVED_MODEL_KEYS` |
> | Docs guard | `docs_and_skills_have_no_dead_model_keys` scans README + every skill |
> | Commit subject | `Refresh model roster against live catalogs (YYYY-MM-DD)` (the audit derives `--since` from it) |
>
> **Entry criteria:** user asks for latest models, a model fails at dispatch, or the weekly cron fires.

The roster rots on its own: Together retires serverless endpoints with no alias, re-ids models
with date suffixes, and keeps dead ids listed. The only ground truth is a real dispatch, so the
audit script probes every configured model the way Squall calls it, then this skill decides what
to change.

## Modes

| Mode | Trigger | Ships via |
|------|---------|-----------|
| **Interactive** (default) | user invokes it | TDD → pre-commit → commit + push `main` → `scripts/deploy-squall.sh` |
| **Unattended** | prompt says "unattended" (weekly launchd job) | same edits in a throwaway worktree → push `main` only after pre-commit passes. Do not deploy (the wrapper does). Never add a new model family. |

`scripts/deploy-squall.sh` builds origin/main and installs a copied binary on every host: the MBP
(`~/.local/bin/squall`, no longer a symlink into the dev checkout), Studio david
(`~/.local/bin/squall`), and Studio ori (`/opt/orchid/ori/.local/bin/squall`, sudo). It verifies each one by hash
and MCP handshake, then records the SHA in `.squall/model-audit/deployed-sha`.

Weekly job: launchd agent `com.squall.latest-weekly` (Mondays 09:00, MBP), installed by
`scripts/install-weekly.sh`. It runs `scripts/squall-latest-weekly.sh` from its own clone at
`~/.squall-latest/squall`: audit → this skill (unattended) → deploy to MBP + Studio david + ori. Logs go
to that clone's `.squall/model-audit/cron.log`, and failures raise a macOS notification.
**launchd can't read `~/Documents`**, and Full Disk Access can't be granted to a script or bare
binary. That's why the job has its own clone, `.env` copy (re-run the installer after rotating
keys), and logs, all outside `~/Documents`.

## Workflow

1. **Audit.** Run `python3 -I scripts/model-audit.py` (or read the report path you were given).
   It writes markdown + JSON to `.squall/model-audit/`. Read the whole markdown report.
2. **Triage every configured model:**
   - **DEAD** → find a successor, in this order: (a) a date-suffixed sibling of the same id
     (`DeepSeek-V4-Pro` → `DeepSeek-V4-Pro-0813`); (b) a newer version in the same family on the
     same provider; (c) the previous version that still serves (2026-11: the Qwen slot fell
     back to `qwen-3.7-max`). No successor → `REMOVED_MODEL_KEYS`.
   - **RATE_LIMITED** → not dead. Upgrade only if a newer same-family model exists anyway.
   - **OK** → upgrade only when a newer same-family model is in the catalog. Same family and
     same provider. Similar or lower price. Probed OK.
   - **Floating aliases need nothing:** `claude` uses `opus` (resolves to the current Opus),
     and the `gemini` entry follows agy's settings.json.
3. **Probe each candidate** before adopting it. Use a streaming request
   (`"stream": true`) because some Together models are streaming-only. Re-probe a 429 with
   backoff before calling it dead.
4. **RED.** Add `roster_matches_<yyyy>_<mm>_live_catalog` (and a per-model test for a
   renamed key) in `src/config.rs` asserting the new keys, model_ids and alias resolutions.
   Run it and confirm it fails.
5. **GREEN.** Edit, in this order:
   1. `BUILTIN_DEFAULTS`: the model entry (key, `model_id`, description, weaknesses with price).
   2. `RETIRED_MODEL_ALIASES`: repoint every existing alias that targeted the old key, then
      add `(old_key, new_key)` and `(old_model_id, new_key)`. If you revert to a key that
      was itself retired, delete its old alias row (`retired_aliases_resolve` fails otherwise).
   3. Synthetic test fixtures that name the old key (`src/dispatch/registry.rs`,
      `src/review.rs`, `tests/smoke.rs`).
   4. README model table and the `TOGETHER_API_KEY` row (display names like "Kimi K3.1 Pro").
   5. Skill prompts in `.claude/skills/*` and `.agents/skills/*`. Ensemble tables and
      `{MODELS}` lines name exact keys.
6. **Verify:** `./scripts/pre-commit.sh`. The docs scanner names any file still using a
   retired key.
7. **Ship** per mode. In interactive mode, also mention anything you deliberately did not adopt
   (new families such as a new Gemini line or a new open-weight model) as candidates.
8. **Learn:** update the Session Learnings below and the `model-roster-staleness` memory with
   what changed and any new provider behavior.

## Provider Gotchas

| Provider | Trap |
|----------|------|
| Together | No "latest" aliases at all (re-verified 2026-11-05). Listing ≠ callable. Bare ids die when a date-suffixed id replaces them. `$0` pricing rows are LoRA/dedicated-only. 429s are dynamic capacity limits, not death. |
| Codex | A local `daybreak = true` in `~/.codex/config.toml` rejects every non-Daybreak model ("Daybreak is unavailable for model X"). The audit probes shipped models with `-c daybreak=false`. The user's overlay pins `gpt-daybreak-blue-latest`. Catalog: `codex debug models`, highest `priority` + `visibility: list`. |
| Claude CLI | `--model` aliases `fable`/`opus`/`sonnet` float. `fable` (Fable 5.1) is the top, pricier tier. Keep `opus` unless the user asks. |
| agy | Status is in-band JSON (`"SUCCESS"`/`"ERROR"`) with exit 0. `--model` needs the full label with its parenthetical. "Individual quota reached" = quota, not dead. |
| xAI | `grok-latest`/`grok-build-latest` can lag. Pin the explicit id from `/v1/models`. |
| OpenRouter | Public `/api/v1/models` `created` timestamps are the best cross-provider "what shipped" signal. Not a dispatch path for Squall. |

## Anti-Patterns

| Don't | Do Instead |
|-------|------------|
| Trust `/v1/models` presence | Probe with a real streaming dispatch |
| Call a 429/quota error "dead" | Back off and re-probe. Mark it RATE_LIMITED. |
| Blind `sed s/"old"/"new"/` across `src/config.rs` | It turns the new `("old","new")` alias rows into self-aliases and rewrites the test's "old" lists. Rename fixtures first, then write the alias rows, or exclude `config.rs`. |
| Rename a key without an alias row | Callers and memory stats keyed on the old name break silently |
| Adopt a new model family unattended | List it as a candidate in the PR body |
| Stop at `git push` | Run `scripts/deploy-squall.sh`. Studio david and ori are copied binaries that rot silently. Studio david sat on a Jul 28 build until 2026-11-05. |
| Expect `listmodels` to show the roster over bare SSH | It lists only models whose key/CLI is available. Remote checks use hash + handshake. |

<!-- SENTINEL:SESSION_LEARNINGS_START - Do not remove this line -->
## Session Learnings Archive

### 2026-11-05: skill created
- Audit found `Qwen/Qwen3.8-2.4T-A95B` removed from Together (500 on dispatch) with only
  Qwen3.8-Flash left, so the slot reverted to `qwen-3.7-max`. Kimi K3 429'd on every probe while
  K3.1-Pro (newer, $2/$10 vs $3/$15) served, so `kimi-k3` → `kimi-k3.1-pro`.
- The docs scanner split "K3.1" into "K3" + "1" and flagged the successor as the retired key.
  Fixed with `version_aware_words` (a `.` between digits stays in the token).
- agy now lists Gemini 4 Argon (Low/Medium/High). It is not adopted yet because it is a new
  line, not a successor to the Flash pin.
<!-- SENTINEL:SESSION_LEARNINGS_END -->
