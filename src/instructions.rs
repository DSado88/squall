//! Server instructions sent at MCP handshake.
//!
//! Two variants exist because Squall runs under different MCP clients:
//! - Claude Code: invokes skills via the Skill tool, spawns Opus subagents via Task,
//!   reads `results_file` after context compaction.
//! - Codex CLI: invokes skills via `$skill-name`, spawns Codex subagents via natural
//!   language, has no compaction-based file rehydration pattern.
//!
//! Selected at runtime by `Config::client_mode` (set via `SQUALL_CLIENT` env var).

use crate::config::ClientMode;

const INSTRUCTIONS_CLAUDE: &str = "Squall: parallel AI model dispatch. Each model is an independent consultant.\n\n\
     FOR CODE REVIEW: Use the `squall-unified-review` skill (invoke via Skill tool), \
        NOT these MCP tools directly. The skill handles depth detection, ensemble selection, \
        Opus agent orchestration, synthesis, and memorization. Calling `review` directly \
        skips all of that.\n\n\
     FOR DIRECT TOOL USE (chat, clink, research — not code review):\n\
     1. FIRST: Call `memory` (recommend/patterns/tactics) to check past learnings.\n\
     2. NEXT: Call `listmodels` for EXACT model names. NEVER hardcode names like \
        \"claude-sonnet\", \"gpt-4\", \"o4-mini\" — these are NOT Squall models. \
        Use ONLY names from `listmodels` output.\n\
     3. ONLY THEN: Call `review`/`chat`/`clink` with the task.\n\
        - Use falsification framing: 'Attempt to PROVE [issue] exists. Report confidence.'\n\
        - Set `deep: true` for security/architecture/high-stakes (600s, high reasoning).\n\
        - `results_file` persists on disk — read it if context compaction loses the response.\n\
     4. Triangulate model findings with your own investigation.\n\
     5. Call `memorize` to capture patterns, tactics, and model recommendations.\n\
     6. After PR merge: `flush` to graduate branch patterns to codebase scope.\n\n\
     DO NOT call `review` without calling `memory` and `listmodels` first.\n\n\
     File context: pass `file_paths` + `working_directory` to include source files.\n\
     For review, also pass `diff` with unified diff text.\n\
     Research: `clink` with model \"codex\" for web search, or `review` with models as advisors.";

const INSTRUCTIONS_CODEX: &str = "Squall: parallel AI model dispatch. Each model is an independent consultant.\n\n\
     FOR CODE REVIEW: Invoke the `$squall-unified-review-codex` skill (Codex skill, lives in `.agents/skills/`), \
        NOT these MCP tools directly. The skill handles depth detection, ensemble selection, \
        Codex subagent orchestration, synthesis, and memorization. Calling `review` directly \
        skips all of that.\n\n\
     FOR DIRECT TOOL USE (chat, clink, research — not code review):\n\
     1. FIRST: Call `memory` (recommend/patterns/tactics) to check past learnings.\n\
     2. NEXT: Call `listmodels` for EXACT model names. NEVER hardcode names like \
        \"gpt-5\", \"claude-sonnet\", \"o4-mini\" — these are NOT Squall models. \
        Use ONLY names from `listmodels` output.\n\
     3. ONLY THEN: Call `review`/`chat`/`clink` with the task.\n\
        - Use falsification framing: 'Attempt to PROVE [issue] exists. Report confidence.'\n\
        - Set `deep: true` for security/architecture/high-stakes (600s, high reasoning).\n\
        - `results_file` persists on disk — read it if you lose the inline response.\n\
     4. Triangulate model findings with your own investigation.\n\
     5. Call `memorize` to capture patterns, tactics, and model recommendations.\n\
     6. After PR merge: `flush` to graduate branch patterns to codebase scope.\n\n\
     DO NOT call `review` without calling `memory` and `listmodels` first.\n\n\
     File context: pass `file_paths` + `working_directory` to include source files.\n\
     For review, also pass `diff` with unified diff text.\n\
     Research: `clink` with model \"claude\" for local Anthropic agent (mirror of how Claude users clink to codex), \
     or `review` with models as advisors.";

pub fn for_client(mode: ClientMode) -> &'static str {
    match mode {
        ClientMode::Claude => INSTRUCTIONS_CLAUDE,
        ClientMode::Codex => INSTRUCTIONS_CODEX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_instructions_reference_skill_tool_and_opus() {
        let txt = for_client(ClientMode::Claude);
        assert!(
            txt.contains("Skill tool"),
            "claude instructions name the Skill tool"
        );
        assert!(
            txt.contains("Opus agent"),
            "claude instructions reference Opus subagent"
        );
    }

    #[test]
    fn codex_instructions_use_dollar_sign_invocation() {
        let txt = for_client(ClientMode::Codex);
        assert!(
            txt.contains("$squall-unified-review-codex"),
            "codex instructions use $skill-name explicit-invoke syntax"
        );
        assert!(
            txt.contains("Codex subagent"),
            "codex instructions reference Codex subagent (not Opus)"
        );
        assert!(
            txt.contains("clink") && txt.contains("\"claude\""),
            "codex instructions point to claude as the clink target (mirror)"
        );
    }
}
