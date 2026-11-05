#!/usr/bin/env python3
"""Audit Squall's model roster against live provider catalogs.

Deterministic half of the /squall-latest skill: it gathers facts, it does not edit
code. Every configured model is probed with a real dispatch, because catalog listing
proves nothing (Together keeps dead models listed and has no "latest" aliases).

Run with `python3 -I scripts/model-audit.py` from the repo root.

Exit codes (the weekly cron wrapper keys off these):
    0   every configured model serves and nothing new appeared since --since
    10  roster healthy, but new catalog entries are worth reviewing
    20  at least one configured model is dead
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

# Families worth surfacing from OpenRouter's catalog: the providers Squall can reach.
OPENROUTER_ORGS = {
    "anthropic", "openai", "x-ai", "google", "moonshotai", "z-ai", "qwen", "deepseek",
    "meta-llama", "mistralai", "minimax",
}


# --------------------------------------------------------------------------- helpers


def load_env() -> None:
    env_file = REPO / ".env"
    if not env_file.is_file():
        return
    for line in env_file.read_text().splitlines():
        m = re.match(r"^\s*(?:export\s+)?([A-Z0-9_]+)=(.*)$", line)
        if m and m.group(1) not in os.environ:
            os.environ[m.group(1)] = m.group(2).strip().strip("'\"")


def builtin_roster() -> dict:
    src = (REPO / "src/config.rs").read_text()
    m = re.search(r'const BUILTIN_DEFAULTS: &str = r#"(.*?)"#;', src, re.S)
    if not m:
        sys.exit("could not find BUILTIN_DEFAULTS in src/config.rs")
    return tomllib.loads(m.group(1))


def overlay_models() -> dict:
    out = {}
    xdg = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    for path in (Path(xdg) / "squall/config.toml", REPO / ".squall/config.toml"):
        if path.is_file():
            for key, entry in tomllib.loads(path.read_text()).get("models", {}).items():
                out[key] = {**entry, "_source": str(path)}
    return out


def http(url: str, *, headers=None, body=None, timeout=60, max_bytes=None):
    """Return (status, text). Never raises for HTTP errors."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers={"User-Agent": UA, **(headers or {})})
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read(max_bytes or -1).decode(errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read(4000).decode(errors="replace")
    except Exception as e:  # timeout, DNS, TLS
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


def short(text: str, n: int = 200) -> str:
    return re.sub(r"\s+", " ", text).strip()[:n]


# --------------------------------------------------------------------------- probes


def probe_http(model_id: str, provider: dict) -> dict:
    key = os.environ.get(provider.get("api_key_env", ""), "")
    if not key:
        return {"status": "NO_KEY", "detail": f"{provider.get('api_key_env')} unset"}
    body = {"model": model_id, "stream": True, "max_tokens": 64,
            "messages": [{"role": "user", "content": PROBE_PROMPT}]}
    detail = ""
    # 429 means "busy", not "dead": back off before judging.
    for wait in (0, 10, 30):
        time.sleep(wait)
        code, text = http(provider["base_url"], headers={"Authorization": f"Bearer {key}"},
                          body=body, timeout=120, max_bytes=4000)
        if code == 200 and '"choices"' in text:
            return {"status": "OK"}
        detail = f"HTTP {code}: {short(text)}"
        if code != 429:
            break
    status = "RATE_LIMITED" if detail.startswith("HTTP 429") else "DEAD"
    return {"status": status, "detail": detail}


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
    code, out, err = run([exe, *args], stdin=PROBE_PROMPT, timeout=240, cwd=str(REPO))
    blob = out + err
    if exe == "codex":
        ok = '"agent_message"' in out
    elif exe == "claude":
        try:
            d = json.loads(out)
            ok = not d.get("is_error") and bool(d.get("result"))
            resolved = list(d.get("modelUsage", {}))
            if ok:
                return {"status": "OK", "resolved": resolved}
        except json.JSONDecodeError:
            ok = False
    elif exe == "agy":
        # Status is reported in-band ("SUCCESS"/"ERROR"), often with exit code 0.
        try:
            ok = str(json.loads(out).get("status", "")).upper() == "SUCCESS"
        except json.JSONDecodeError:
            ok = False
    else:
        ok = code == 0
    if ok:
        return {"status": "OK", **({"note": "probed with daybreak=false"} if extra else {})}
    # Subscription CLIs report exhausted quota as an error; the model itself is fine.
    if re.search(r"quota|rate.?limit|usage limit|too many requests", blob, re.I):
        return {"status": "RATE_LIMITED", "detail": f"exit {code}: {short(blob, 300)}"}
    return {"status": "DEAD", "detail": f"exit {code}: {short(blob, 300)}"}


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
        return {"status": "DEAD" if code == 404 else "UNVERIFIED",
                "detail": f"HTTP {code}: {short(text)}"}
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


# --------------------------------------------------------------------------- catalogs


def together_catalog() -> list[dict]:
    key = os.environ.get("TOGETHER_API_KEY")
    if not key:
        return []
    code, text = http("https://api.together.xyz/v1/models",
                      headers={"Authorization": f"Bearer {key}"})
    if code != 200:
        return []
    data = json.loads(text)
    return data if isinstance(data, list) else data.get("data", [])


def openrouter_catalog() -> list[dict]:
    code, text = http("https://openrouter.ai/api/v1/models")
    return json.loads(text).get("data", []) if code == 200 else []


def codex_catalog() -> list[dict]:
    code, out, _ = run(["codex", "debug", "models"], timeout=60)
    if code != 0:
        return []
    data = json.loads(out)
    return data.get("models", data) if isinstance(data, dict) else data


def agy_catalog() -> list[str]:
    code, out, _ = run(["agy", "models"], timeout=60)
    return [l.split("\t", 1)[1] for l in out.splitlines() if "\t" in l] if code == 0 else []


def xai_catalog() -> list[str]:
    key = os.environ.get("XAI_API_KEY")
    if not key:
        return []
    code, text = http("https://api.x.ai/v1/models", headers={"Authorization": f"Bearer {key}"})
    return sorted(m["id"] for m in json.loads(text).get("data", [])) if code == 200 else []


def claude_aliases() -> dict:
    """Resolve the Claude CLI's floating aliases to concrete model ids."""
    out = {}
    for alias in ("fable", "opus", "sonnet"):
        code, stdout, _ = run(["claude", "--print", "--output-format", "json",
                               "--model", alias], stdin="Reply OK", timeout=120,
                              cwd=str(Path.home()))
        try:
            out[alias] = list(json.loads(stdout).get("modelUsage", {}))
        except json.JSONDecodeError:
            out[alias] = [f"error (exit {code})"]
    return out


# --------------------------------------------------------------------------- main


def default_since() -> str:
    code, out, _ = run(["git", "log", "-1", "--format=%cs", "--grep=^Refresh model roster"],
                       cwd=str(REPO), timeout=30)
    return out.strip() or (dt.date.today() - dt.timedelta(days=30)).isoformat()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--since", default=None,
                    help="report catalog entries newer than this date "
                         "(default: date of the last 'Refresh model roster' commit)")
    ap.add_argument("--out", default=str(REPO / ".squall/model-audit"))
    ap.add_argument("--skip-claude-aliases", action="store_true",
                    help="skip resolving fable/opus/sonnet (saves 3 CLI calls)")
    args = ap.parse_args()

    load_env()
    since = args.since or default_since()
    since_ts = dt.datetime.fromisoformat(since).timestamp()
    today = dt.date.today().isoformat()

    roster = builtin_roster()
    providers = roster.get("providers", {})
    models = roster.get("models", {})
    overlays = overlay_models()

    results = {k: probe(k, e, providers) for k, e in models.items()}
    overlay_results = {k: probe(k, e, providers) for k, e in overlays.items()}

    together = together_catalog()
    tg_ids = {m["id"] for m in together}
    tg_checks = {}
    for key, e in models.items():
        if e.get("provider") == "together":
            mid = e["model_id"]
            base = re.sub(r"-\d{4}$", "", mid)
            tg_checks[key] = {
                "listed": mid in tg_ids,
                # Together re-ids with date suffixes and drops the bare id.
                "siblings": sorted(i for i in tg_ids if i != mid and i.startswith(base)),
            }
    tg_new = sorted(
        (m for m in together
         if m.get("type") in ("chat", "language") and m.get("created", 0) >= since_ts
         and (m.get("pricing") or {}).get("input", 0) > 0),  # $0 = LoRA/dedicated-only
        key=lambda m: -m.get("created", 0))
    or_new = sorted(
        (m for m in openrouter_catalog()
         if m.get("created", 0) >= since_ts and m["id"].split("/")[0] in OPENROUTER_ORGS
         and ":" not in m["id"]),
        key=lambda m: -m.get("created", 0))
    codex = [m for m in codex_catalog() if m.get("visibility") == "list"]
    agy = agy_catalog()
    xai = xai_catalog()
    aliases = {} if args.skip_claude_aliases else claude_aliases()

    dead = [k for k, r in results.items() if r["status"] == "DEAD"]
    configured_ids = {e.get("model_id") for e in models.values()}
    agy_new = [label for label in agy if label not in configured_ids]
    new_anything = bool(tg_new or or_new)

    # ---------------------------------------------------------------- report
    lines = [f"# Squall model audit {today}", "",
             f"New-model window: since {since}.", "", "## Configured roster (BUILTIN_DEFAULTS)", "",
             "| key | model_id | backend | status | detail |", "|---|---|---|---|---|"]
    for k, e in models.items():
        r = results[k]
        detail = r.get("detail") or r.get("note") or ", ".join(r.get("resolved", []))
        if k in tg_checks:
            c = tg_checks[k]
            detail += f" listed={c['listed']}" + (f" siblings={c['siblings']}" if c["siblings"] else "")
        lines.append(f"| {k} | `{e.get('model_id')}` | {e.get('backend')} | **{r['status']}** | {short(detail, 160)} |")
    if overlays:
        lines += ["", "## Local overlays (not shipped)", "",
                  "| key | model_id | status | detail | source |", "|---|---|---|---|---|"]
        for k, e in overlays.items():
            r = overlay_results[k]
            lines.append(f"| {k} | `{e.get('model_id')}` | **{r['status']}** | {short(r.get('detail', ''), 120)} | {e['_source']} |")
    lines += ["", f"## Together: new serverless chat models since {since}", ""]
    lines += [f"- {dt.date.fromtimestamp(m['created'])} `{m['id']}` ctx={m.get('context_length')} "
              f"${m['pricing'].get('input')}/${m['pricing'].get('output')} per M" for m in tg_new] or ["- none"]
    lines += ["", f"## OpenRouter: new models since {since} (release signal across providers)", ""]
    lines += [f"- {dt.date.fromtimestamp(m['created'])} `{m['id']}` ctx={m.get('context_length')}"
              for m in or_new] or ["- none"]
    lines += ["", "## Codex CLI catalog (`codex debug models`, visible)", ""]
    lines += [f"- `{m['slug']}` p{m.get('priority')}: {m.get('description', '')}" for m in codex] or ["- unavailable"]
    lines += ["", "## Antigravity (`agy models`)", ""]
    lines += [f"- {label}" + ("" if label in configured_ids else "  ← not configured") for label in agy] or ["- unavailable"]
    if aliases:
        lines += ["", "## Claude CLI aliases", ""]
        lines += [f"- `{a}` → {', '.join(ids)}" for a, ids in aliases.items()]
    if xai:
        lines += ["", "## xAI catalog", "", "- " + ", ".join(f"`{i}`" for i in xai)]
    lines += ["", "## Verdict", "",
              f"- dead: {', '.join(dead) or 'none'}",
              f"- new catalog entries: {'yes' if new_anything else 'none'}"]

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    md = out_dir / f"{today}.md"
    md.write_text("\n".join(lines) + "\n")
    (out_dir / f"{today}.json").write_text(json.dumps({
        "date": today, "since": since, "probes": results, "overlay_probes": overlay_results,
        "together_checks": tg_checks, "together_new": tg_new, "openrouter_new": or_new,
        "codex": codex, "agy": agy, "agy_unconfigured": agy_new, "claude_aliases": aliases,
        "xai": xai, "dead": dead,
    }, indent=2, default=str))
    print(md)
    return 20 if dead else 10 if new_anything else 0


if __name__ == "__main__":
    sys.exit(main())
