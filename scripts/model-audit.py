#!/usr/bin/env python3
"""Audit Squall's model roster against live provider catalogs.

Deterministic half of the /squall-latest skill: it gathers facts, it does not edit
code. Every configured model is probed with a real dispatch, because catalog listing
proves nothing (Together keeps dead models listed and has no "latest" aliases).

Run with `python3 -I scripts/model-audit.py` from the repo root.

Exit codes (the weekly wrapper keys off these):
    0   every configured model serves and nothing new appeared
    10  roster healthy, but new catalog entries are worth reviewing
    20  at least one configured model is definitively gone (provider said so)
    30  inconclusive: a probe or catalog could not be checked (network, auth, missing
        key or CLI, timeout). Needs a human, never the unattended agent, because acting
        on it would "replace" models that are fine.

Only a provider's explicit "this model does not exist" is DEAD. Everything that might
be our side (transport, 5xx, auth, timeouts, unparseable output) is UNVERIFIED/NO_AUTH.

Everything copied from a provider into the report is sanitized: the report is read by
an unattended agent, so catalog text must not be able to carry instructions.

`--roster-keys FILE|-` prints the BUILTIN_DEFAULTS model keys from a config.rs (used by
deploy-squall.sh) and fails loudly if it can't find any.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PROBE_PROMPT = "Reply with just OK"
UA = "squall-model-audit/1"
RETRY_WAITS = (0, 10, 30)
CLI_TIMEOUT = 240

EXIT_HEALTHY, EXIT_NEW, EXIT_DEAD, EXIT_INCONCLUSIVE = 0, 10, 20, 30
INCONCLUSIVE = {"NO_KEY", "NO_AUTH", "UNVERIFIED"}

# Families worth surfacing from OpenRouter's catalog: the providers Squall can reach.
OPENROUTER_ORGS = {
    "anthropic", "openai", "x-ai", "google", "moonshotai", "z-ai", "qwen", "deepseek",
    "meta-llama", "mistralai", "minimax",
}

# A provider or CLI saying the model itself is gone. Anything else is not proof.
MODEL_GONE = re.compile(
    r"(unknown|unrecognized|invalid|unsupported|nonexistent|no such)\s+model"
    r"|model\b[^\n]{0,80}?\b(not found|does not exist|is not supported|not supported"
    r"|unrecognized|not available|has been (?:retired|deprecated|removed))"
    r"|model_not_found|non-serverless model",
    re.I,
)
ID_RE = re.compile(r"^[A-Za-z0-9._:/ ()+@-]{1,120}$")


# --------------------------------------------------------------------------- helpers


def clean_id(value) -> str:
    """Pass a provider id through only if it looks like an id."""
    s = str(value)
    return s if ID_RE.match(s) else "[invalid id]"


def clean_text(value, n: int = 160) -> str:
    """Flatten untrusted provider text to plain words: no markup, quotes or newlines."""
    s = re.sub(r"[^A-Za-z0-9 .,;:()'/+%=_@-]", " ", str(value))
    return re.sub(r"\s+", " ", s).strip()[:n]


def load_env() -> None:
    env_file = REPO / ".env"
    if not env_file.is_file():
        return
    for line in env_file.read_text().splitlines():
        m = re.match(r"^\s*(?:export\s+)?([A-Z0-9_]+)=(.*)$", line)
        if m and m.group(1) not in os.environ:
            os.environ[m.group(1)] = m.group(2).strip().strip("'\"")


def roster_from_source(src: str) -> dict:
    m = re.search(r'const BUILTIN_DEFAULTS: &str = r#"(.*?)"#;', src, re.S)
    if not m:
        raise ValueError("BUILTIN_DEFAULTS not found")
    roster = tomllib.loads(m.group(1))
    if not roster.get("models"):
        raise ValueError("BUILTIN_DEFAULTS has no models")
    return roster


def builtin_roster() -> dict:
    try:
        return roster_from_source((REPO / "src/config.rs").read_text())
    except (OSError, ValueError, tomllib.TOMLDecodeError) as e:
        sys.exit(f"could not read the roster from src/config.rs: {e}")


def overlay_models() -> dict:
    out = {}
    xdg = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    for path in (Path(xdg) / "squall/config.toml", REPO / ".squall/config.toml"):
        if path.is_file():
            for key, entry in tomllib.loads(path.read_text()).get("models", {}).items():
                out[key] = {**entry, "_source": str(path)}
    return out


def http(url: str, *, headers=None, body=None, timeout=60, max_bytes=None):
    """Return (status, text). Never raises; status 0 means no HTTP response at all."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers={"User-Agent": UA, **(headers or {})})
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read(max_bytes or -1).decode(errors="replace")
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.read(4000).decode(errors="replace")
    except Exception as e:  # timeout, DNS, TLS, refused
        return 0, f"{type(e).__name__}: {e}"


def run(cmd: list[str], stdin: str = "", timeout: int = 180, cwd: str | None = None):
    try:
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True,
                           timeout=timeout, cwd=cwd)
        return p.returncode, p.stdout, p.stderr
    except FileNotFoundError:
        return 127, "", f"{cmd[0]} not found"
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {timeout}s"
    except OSError as e:
        return 126, "", f"{cmd[0]}: {e}"


def parse_json(text: str):
    try:
        return json.loads(text)
    except ValueError:
        return None


# --------------------------------------------------------------------------- probes


def http_retryable(code: int, text: str) -> bool:
    if code == 200:
        return True  # a 200 without "choices" is an odd/partial answer: ask again
    return code in (0, 408, 429) or code >= 500


def classify_http_failure(code: int, text: str) -> str:
    if code == 429:
        return "RATE_LIMITED"
    if code in (401, 403):
        return "NO_AUTH"
    if 400 <= code < 500 and code != 408 and (code == 404 or MODEL_GONE.search(text)):
        return "DEAD"
    return "UNVERIFIED"


def probe_http(model_id: str, provider: dict) -> dict:
    key = os.environ.get(provider.get("api_key_env", ""), "")
    if not key:
        return {"status": "NO_KEY", "detail": f"{provider.get('api_key_env')} unset"}
    body = {"model": model_id, "stream": True, "max_tokens": 64,
            "messages": [{"role": "user", "content": PROBE_PROMPT}]}
    code, text = 0, ""
    for wait in RETRY_WAITS:
        time.sleep(wait)
        code, text = http(provider["base_url"], headers={"Authorization": f"Bearer {key}"},
                          body=body, timeout=120, max_bytes=4000)
        if code == 200 and '"choices"' in text:
            return {"status": "OK"}
        if not http_retryable(code, text):
            break
    return {"status": classify_http_failure(code, text),
            "detail": f"HTTP {code}: {clean_text(text)}"}


def probe_cli(key: str, entry: dict) -> dict:
    exe = entry.get("executable", "")
    args = [a.replace("{model}", entry.get("model_id", "")).replace("{reasoning}", "low")
            for a in entry.get("args_template", [])]
    extra = []
    if exe == "codex":
        # A local `daybreak = true` rejects every non-Daybreak model. Probe what ships,
        # not this machine's codex preference.
        if "daybreak" not in entry.get("model_id", ""):
            extra = ["-c", "daybreak=false"]
        args = args[:1] + extra + args[1:]
    code, out, err = run([exe, *args], stdin=PROBE_PROMPT, timeout=CLI_TIMEOUT, cwd=str(REPO))
    blob = out + err
    detail = f"exit {code}: {clean_text(blob, 240)}"
    if code in (124, 126, 127):
        # Missing, unrunnable or hung CLI: our environment, not the model.
        return {"status": "UNVERIFIED", "detail": detail}
    if exe == "codex":
        ok = '"agent_message"' in out
    elif exe == "claude":
        d = parse_json(out)
        ok = isinstance(d, dict) and not d.get("is_error") and bool(d.get("result"))
        if ok:
            return {"status": "OK",
                    "resolved": [clean_id(m) for m in d.get("modelUsage", {})]}
    elif exe == "agy":
        # Status is reported in-band ("SUCCESS"/"ERROR"), often with exit code 0.
        d = parse_json(out)
        ok = isinstance(d, dict) and str(d.get("status", "")).upper() == "SUCCESS"
    else:
        ok = code == 0
    if ok:
        return {"status": "OK", **({"note": "probed with daybreak=false"} if extra else {})}
    # Subscription CLIs report exhausted quota as an error; the model itself is fine.
    if re.search(r"quota|rate.?limit|usage limit|too many requests", blob, re.I):
        return {"status": "RATE_LIMITED", "detail": detail}
    if MODEL_GONE.search(blob):
        return {"status": "DEAD", "detail": detail}
    return {"status": "UNVERIFIED", "detail": detail}


def probe_async(entry: dict) -> dict:
    if entry.get("async_poll_type") == "gemini_interactions":
        key = os.environ.get(entry.get("api_key_env", ""), "")
        if not key:
            return {"status": "NO_KEY", "detail": f"{entry.get('api_key_env')} unset"}
        # Launching a research job costs money; a metadata lookup proves the id exists.
        code, text = http("https://generativelanguage.googleapis.com/v1beta/models/"
                          f"{entry['model_id']}", headers={"x-goog-api-key": key})
        if code == 200:
            return {"status": "OK", "note": "metadata only, not dispatched"}
        status = ("DEAD" if code == 404 else "NO_AUTH" if code in (401, 403)
                  else "UNVERIFIED")
        return {"status": status, "detail": f"HTTP {code}: {clean_text(text)}"}
    return {"status": "UNVERIFIED", "detail": "unknown async_poll_type"}


def probe(key: str, entry: dict, providers: dict) -> dict:
    backend = entry.get("backend")
    if backend == "http":
        prov = providers.get(entry.get("provider"), {})
        if not prov:
            return {"status": "UNVERIFIED", "detail": f"no provider {entry.get('provider')}"}
        return probe_http(entry["model_id"], prov)
    if backend == "cli":
        return probe_cli(key, entry)
    if backend == "async_poll":
        return probe_async(entry)
    return {"status": "UNVERIFIED", "detail": f"backend {backend}"}


def verdict(probes: dict, catalog_errors: list, new_entries: bool) -> int:
    statuses = {r["status"] for r in probes.values()}
    if catalog_errors or statuses & INCONCLUSIVE:
        return EXIT_INCONCLUSIVE
    if "DEAD" in statuses:
        return EXIT_DEAD
    return EXIT_NEW if new_entries else EXIT_HEALTHY


# --------------------------------------------------------------------------- catalogs
# Each returns (items, error). An error makes the audit inconclusive instead of
# silently reporting "nothing new".


def together_catalog():
    key = os.environ.get("TOGETHER_API_KEY")
    if not key:
        return [], "together: TOGETHER_API_KEY unset"
    code, text = http("https://api.together.xyz/v1/models",
                      headers={"Authorization": f"Bearer {key}"})
    if code != 200:
        return [], f"together: HTTP {code}"
    data = parse_json(text)
    items = data if isinstance(data, list) else (data or {}).get("data") if isinstance(data, dict) else None
    if not isinstance(items, list) or not items:
        return [], "together: unreadable or empty catalog"
    return [m for m in items if isinstance(m, dict) and "id" in m], None


def openrouter_catalog():
    code, text = http("https://openrouter.ai/api/v1/models")
    if code != 200:
        return [], f"openrouter: HTTP {code}"
    data = parse_json(text)
    items = data.get("data") if isinstance(data, dict) else None
    if not isinstance(items, list) or not items:
        return [], "openrouter: unreadable or empty catalog"
    return [m for m in items if isinstance(m, dict) and "id" in m], None


def codex_catalog():
    code, out, _ = run(["codex", "debug", "models"], timeout=60)
    if code != 0:
        return [], f"codex debug models: exit {code}"
    data = parse_json(out)
    items = data.get("models") if isinstance(data, dict) else data
    if not isinstance(items, list) or not items:
        return [], "codex debug models: unreadable output"
    return [m for m in items if isinstance(m, dict) and "slug" in m], None


def agy_catalog():
    code, out, _ = run(["agy", "models"], timeout=60)
    labels = [l.split("\t", 1)[1].strip() for l in out.splitlines() if "\t" in l]
    if code != 0 or not labels:
        return [], f"agy models: exit {code}"
    return [clean_id(l) for l in labels], None


def xai_catalog():
    key = os.environ.get("XAI_API_KEY")
    if not key:
        return [], "xai: XAI_API_KEY unset"
    code, text = http("https://api.x.ai/v1/models", headers={"Authorization": f"Bearer {key}"})
    data = parse_json(text) if code == 200 else None
    items = data.get("data") if isinstance(data, dict) else None
    if not isinstance(items, list) or not items:
        return [], f"xai: HTTP {code} or unreadable catalog"
    return sorted(clean_id(m.get("id", "")) for m in items if isinstance(m, dict)), None


def claude_aliases() -> dict:
    """Resolve the Claude CLI's floating aliases to concrete model ids (informational)."""
    out = {}
    for alias in ("fable", "opus", "sonnet"):
        code, stdout, _ = run(["claude", "--print", "--output-format", "json",
                               "--model", alias], stdin="Reply OK", timeout=120,
                              cwd=str(Path.home()))
        d = parse_json(stdout)
        out[alias] = ([clean_id(m) for m in d.get("modelUsage", {})] if isinstance(d, dict)
                      else [f"error (exit {code})"])
    return out


def unseen(current: list, seen: list | None) -> list:
    """Entries added since the last review. No baseline yet means nothing is new."""
    if seen is None:
        return []
    return sorted(set(current) - set(seen))


def slim_together(m: dict) -> dict:
    pricing = m.get("pricing") or {}
    return {"id": clean_id(m.get("id", "")), "created": m.get("created"),
            "context_length": m.get("context_length"),
            "pricing": {"input": pricing.get("input"), "output": pricing.get("output")}}


def slim_openrouter(m: dict) -> dict:
    return {"id": clean_id(m.get("id", "")), "created": m.get("created"),
            "context_length": m.get("context_length")}


def slim_codex(m: dict) -> dict:
    return {"slug": clean_id(m.get("slug", "")), "priority": m.get("priority"),
            "description": clean_text(m.get("description", ""), 100)}


# --------------------------------------------------------------------------- main


def default_since(reviewed_at: str | None = None) -> str:
    """Later of the last roster-refresh commit and the last unattended review."""
    code, out, _ = run(["git", "log", "-1", "--format=%cI", "--grep=^Refresh model roster"],
                       cwd=str(REPO), timeout=30)
    candidates = [c for c in (out.strip(), reviewed_at) if c]
    if not candidates:
        return (dt.date.today() - dt.timedelta(days=30)).isoformat()
    return max(candidates, key=lambda c: dt.datetime.fromisoformat(c).timestamp())


def roster_keys_cli(path: str) -> int:
    try:
        src = sys.stdin.read() if path == "-" else Path(path).read_text()
        keys = list(roster_from_source(src)["models"])
    except (OSError, ValueError, tomllib.TOMLDecodeError) as e:
        print(f"model-audit: no roster keys: {e}", file=sys.stderr)
        return 2
    print("\n".join(keys))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--since", default=None,
                    help="report catalog entries newer than this ISO date/time "
                         "(default: see --reviewed-at)")
    ap.add_argument("--reviewed-at", default=None,
                    help="ISO time the catalogs were last reviewed; the default window "
                         "starts at the later of this and the last roster commit")
    ap.add_argument("--seen", default=None,
                    help="JSON of catalog entries already reviewed (agy/codex/xai have no "
                         "timestamps, so additions are found by set difference)")
    ap.add_argument("--out", default=str(REPO / ".squall/model-audit"))
    ap.add_argument("--skip-claude-aliases", action="store_true",
                    help="skip resolving fable/opus/sonnet (saves 3 CLI calls)")
    ap.add_argument("--roster-keys", metavar="FILE",
                    help="print BUILTIN_DEFAULTS model keys from FILE ('-' = stdin) and exit")
    args = ap.parse_args()
    if args.roster_keys:
        return roster_keys_cli(args.roster_keys)

    load_env()
    since = args.since or default_since(args.reviewed_at)
    since_ts = dt.datetime.fromisoformat(since).timestamp()
    now = time.strftime("%Y-%m-%d %H:%M:%S")

    roster = builtin_roster()
    providers = roster.get("providers", {})
    models = roster.get("models", {})
    overlays = overlay_models()

    results = {k: probe(k, e, providers) for k, e in models.items()}
    overlay_results = {k: probe(k, e, providers) for k, e in overlays.items()}

    errors = []
    together, err = together_catalog()
    errors += [err] if err else []
    openrouter, err = openrouter_catalog()
    errors += [err] if err else []
    codex_all, err = codex_catalog()
    errors += [err] if err else []
    agy, err = agy_catalog()
    errors += [err] if err else []
    xai, err = xai_catalog()
    errors += [err] if err else []
    aliases = {} if args.skip_claude_aliases else claude_aliases()

    tg_ids = {m["id"] for m in together}
    tg_checks = {}
    for key, e in models.items():
        if e.get("provider") == "together" and tg_ids:
            mid = e["model_id"]
            base = re.sub(r"-\d{4}$", "", mid)
            tg_checks[key] = {
                "listed": mid in tg_ids,
                # Together re-ids with date suffixes and drops the bare id.
                "siblings": sorted(clean_id(i) for i in tg_ids
                                   if i != mid and i.startswith(base)),
            }
    tg_new = [slim_together(m) for m in sorted(
        (m for m in together
         if m.get("type") in ("chat", "language") and (m.get("created") or 0) >= since_ts
         and ((m.get("pricing") or {}).get("input") or 0) > 0),  # $0 = LoRA/dedicated
        key=lambda m: -(m.get("created") or 0))]
    or_new = [slim_openrouter(m) for m in sorted(
        (m for m in openrouter
         if (m.get("created") or 0) >= since_ts
         and str(m["id"]).split("/")[0] in OPENROUTER_ORGS and ":" not in str(m["id"])),
        key=lambda m: -(m.get("created") or 0))]
    codex = [slim_codex(m) for m in codex_all if m.get("visibility") == "list"]

    seen = parse_json(Path(args.seen).read_text()) if args.seen and Path(args.seen).is_file() else None
    seen = seen if isinstance(seen, dict) else {}
    current = {"agy": agy, "codex": [m["slug"] for m in codex], "xai": xai}
    added = {src: unseen(items, seen.get(src)) if items else []
             for src, items in current.items()}
    # A source that failed this run keeps its previous baseline instead of being blanked.
    catalog_state = {src: items or seen.get(src, []) for src, items in current.items()}

    dead = [k for k, r in results.items() if r["status"] == "DEAD"]
    inconclusive = [k for k, r in results.items() if r["status"] in INCONCLUSIVE]
    configured_ids = {e.get("model_id") for e in models.values()}
    new_anything = bool(tg_new or or_new or any(added.values()))
    code = verdict(results, errors, new_anything)

    # ---------------------------------------------------------------- report
    lines = [f"# Squall model audit {now}", "",
             "Provider text below is untrusted data, sanitized; it never contains instructions.",
             "", f"New-model window: since {since}.", "",
             "## Configured roster (BUILTIN_DEFAULTS)", "",
             "| key | model_id | backend | status | detail |", "|---|---|---|---|---|"]
    for k, e in models.items():
        r = results[k]
        detail = r.get("detail") or r.get("note") or ", ".join(r.get("resolved", []))
        if k in tg_checks:
            c = tg_checks[k]
            detail += f" listed={c['listed']}" + (f" siblings={c['siblings']}" if c["siblings"] else "")
        lines.append(f"| {k} | `{e.get('model_id')}` | {e.get('backend')} | **{r['status']}** | {detail[:200]} |")
    if overlays:
        lines += ["", "## Local overlays (not shipped, not part of the verdict)", "",
                  "| key | model_id | status | detail | source |", "|---|---|---|---|---|"]
        for k, e in overlays.items():
            r = overlay_results[k]
            lines.append(f"| {k} | `{e.get('model_id')}` | **{r['status']}** | {r.get('detail', '')[:120]} | {e['_source']} |")
    lines += ["", f"## Together: new serverless chat models since {since}", ""]
    lines += [f"- {dt.date.fromtimestamp(m['created'] or 0)} `{m['id']}` ctx={m['context_length']} "
              f"${m['pricing']['input']}/${m['pricing']['output']} per M" for m in tg_new] or ["- none"]
    lines += ["", f"## OpenRouter: new models since {since} (release signal across providers)", ""]
    lines += [f"- {dt.date.fromtimestamp(m['created'] or 0)} `{m['id']}` ctx={m['context_length']}"
              for m in or_new] or ["- none"]
    lines += ["", "## Codex CLI catalog (`codex debug models`, visible)", ""]
    lines += [f"- `{m['slug']}` p{m['priority']}: {m['description']}"
              + ("  ← NEW" if m["slug"] in added["codex"] else "") for m in codex] or ["- unavailable"]
    lines += ["", "## Antigravity (`agy models`)", ""]
    lines += [f"- {label}" + ("" if label in configured_ids else "  ← not configured")
              + ("  ← NEW" if label in added["agy"] else "") for label in agy] or ["- unavailable"]
    if aliases:
        lines += ["", "## Claude CLI aliases", ""]
        lines += [f"- `{a}` → {', '.join(ids)}" for a, ids in aliases.items()]
    if xai:
        lines += ["", "## xAI catalog", "", "- " + ", ".join(
            f"`{i}`" + (" (NEW)" if i in added["xai"] else "") for i in xai)]
    lines += ["", "## Verdict", "",
              f"- exit {code} ({ {0: 'healthy', 10: 'new entries', 20: 'dead model', 30: 'inconclusive'}[code] })",
              f"- dead: {', '.join(dead) or 'none'}",
              f"- could not verify: {', '.join(inconclusive) or 'none'}",
              f"- catalog errors: {'; '.join(errors) or 'none'}",
              f"- new catalog entries: {'yes' if new_anything else 'none'}"]

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = out_dir / time.strftime("%Y-%m-%d-%H%M%S")
    md = stem.with_suffix(".md")
    md.write_text("\n".join(lines) + "\n")
    stem.with_suffix(".json").write_text(json.dumps({
        "time": now, "since": since, "exit": code, "probes": results,
        "overlay_probes": overlay_results, "catalog_errors": errors,
        "together_checks": tg_checks, "together_new": tg_new, "openrouter_new": or_new,
        "codex": codex, "agy": agy, "xai": xai, "added": added,
        "claude_aliases": aliases, "dead": dead, "inconclusive": inconclusive,
    }, indent=2, default=str))
    Path(f"{stem}.catalog.json").write_text(json.dumps(catalog_state, indent=2))
    print(md)
    return code


if __name__ == "__main__":
    sys.exit(main())
