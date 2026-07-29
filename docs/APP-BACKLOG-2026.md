# RUUAH app slices — the homegrown map from Warp OSS and Superset

Written 2026-07-29 (IDT), researched live the same day. `BACKLOG-2026.md` is the
*protocol* backlog (what the VT layer owes any terminal); this file is the *app*
backlog — what makes RUUAH a product rather than a viewport. Sources, verified
2026-07-29 via web + browser cross-confirmation:

- **Warp** went open source 2026-04-28: the whole client is at
  `github.com/warpdotdev/warp` under **AGPL v3** (the `warpui`/`warpui_core` UI
  crates under MIT). Oz (cloud agent orchestration) and Warp Drive stay closed.
  Rust; credits Tokio, Alacritty, NuShell, Fig completion specs.
- **Superset** (`superset.sh`, github under **Elastic License 2.0**): macOS app
  running many CLI agents in parallel, each in its own git worktree, with a
  unified diff-review dashboard, persistent sessions, scheduled automations and
  an MCP server for remote control.

## The two laws of this file

1. **HOMEGROWN, CLEAN-ROOM.** Everything is built here in Rust + Swift the way
   the terminal itself was. **Never copy, port, or paraphrase code from
   `warpdotdev/warp`** — AGPL v3 in a private repo is contamination, and ELv2
   is no better. We study *behavior* from docs and the product, then implement
   from our own architecture. The one importable artifact is **data**: Fig's
   completion *specs* (MIT) are declarative JSON we may consume with our own
   engine.
2. **THE IRON RULES HOLD.** Harness before slice; controls seen to fail; the
   core stays pure (no I/O); bidi stays in the renderer. App slices live in
   `crates/host` + `swift/` unless a real observable forces a core seam — and
   then the corpus pins it.

## What RUUAH already has that these products build on

- **OSC 133 semantic zones in the core** (`semantic.rs`, slice 5.6): prompt /
  input / output regions, corpus-pinned. This is the exact substrate Warp's
  signature Blocks feature needs — most terminals bolt it on; we start with it.
- **Session sidebar** (2026-07-29): one `RuuahHost` per session, background
  sessions keep running. This is the seed of Superset's workspace list.
- **Frame mode-bits + reported background**: the pattern for any state the GUI
  needs from the terminal without touching the pump thread.

## The slices, ordered

**S1 — Settings file + themes.** Warp's settings file + terminal appearance,
and Orel's stated want. A TOML at `~/.ruuah/config.toml`: font, size, theme,
keybinds, default shell. A theme = a palette file (16 ANSI + fg/bg/cursor);
`Palette` already resolves everything through `default_*` fields, and the
margin already follows the reported background — themes drop in. Shape:
host gains `ruuah_host_spawn` options for palette (or a set-palette call);
Swift reads the TOML. Oracle: a theme file with a distinct background must
recolor grid AND margin in a window capture; a bad file must fall back loudly.
*Prereq for S2+ (keybinds live in the settings).*

**S2 — Blocks v1. LANDED 2026-07-29, one compat gap open.** The C surface
carries per-row OSC 133 classes (`RuuahHostFrame.row_semantics`) and filtered
row text (`ruuah_host_row_text` — the input filter is what makes copy-command
return `ls -la` out of `$ ls -la`; both pinned in host_abi with a no-marks
control). Swift: gutter bars per block in the left margin, click pops copy
command / copy output / run again; `shell/ruuah-integration.zsh` +
`shell/zdotdir/.zshenv` bootstrap wired via ZDOTDIR env in the .app. Proven:
scripted two-command child shows two bars with the boundary at the block seam
(`docs/images/blocks-gutter-20260729.png`, pixel-scanned), and a PRISTINE zsh
through the real integration shows the live prompt bar.
**OPEN — real-config compat:** in Orel's own shell (starship transient prompt
+ iTerm2 shell integration) no rows classify. Suspects, measured 2026-07-29:
starship's precmd regenerates PROMPT after ours (wipes the B mark);
iTerm2's integration interleaves `133;C;`/`133;D;0` (marks WITH options) whose
interplay with our strict parser is unmeasured. Next round: instrument
(log unparsed OSC 133 forms), measure upstream's exact option tolerance, and
decide mark precedence. Until then blocks light up only without those
frameworks. *V2 later: sticky command header, block search, collapse.*

**S3 — Command palette + workflows.** cmd+K palette (Swift, pure UI): actions
(new session, switch, copy block, theme switch) + **workflows** = parameterized
command templates in YAML/TOML under `~/.ruuah/workflows/` (Warp's shape;
plain data, our format). Picking one fills the input with placeholders. Cheap,
high daily value. Oracle: unit-test the placeholder substitution; palette
exercised by hand (aesthetics = Orel).

**S4 — History autosuggestions, then completions.** Fish-style ghost text from
per-machine command history (host-side history store keyed by cwd — we see
every `send`, or via shell integration precmd). Accept with →. Phase two:
a completion engine consuming Fig's MIT spec JSON (our parser, our ranking).
Oracle: deterministic history fixture → expected suggestion; specs round-trip
tests. *Depends on S2's shell integration for clean command boundaries.*

**S5 — Worktree workspaces (the Superset/convoy shape).** "New workspace" =
create a git worktree of the current repo + spawn a session (optionally
launching a CLI agent — claude, codex) inside it. Sidebar groups sessions by
workspace; closing a workspace offers to remove the worktree. This is
**convoy's thesis** (tools/convoy, Orel's own prior art) — Superset shipping it
as a product validates the direction; RUUAH absorbing it is Orel's call on
convoy's future. Oracle: spawn → `git worktree list` shows it; close → gone;
two agents editing in parallel never touch each other's tree.

**S6 — Diff review panel.** Superset's dashboard, one workspace at a time:
changed files + unified diff, refreshed on demand (`git status`/`diff`
host-side, rendered Swift-side), "open in editor" per file. Pairs with S5.
Oracle: scripted mutation in a worktree must appear in the panel byte-exact.

**S7 — Splits and tabs: REFUSED for now (Orel, 2026-07-29).** The sidebar IS
the window management: each session is its own full terminal surface, switched
instantly from the list, closed with its row's X. One thing on screen at a
time is the model. Revisit only if real daily use produces the sentence "I
need these two side by side" — and then as a deliberate decision, not a drift.

**S8 — Persistent sessions.** Superset's "closing the laptop doesn't kill the
sessions". V1 exists today (sessions outlive their window inside the app).
V2 — sessions survive the app relaunching — means a detached host process
owning the ptys with the app reattaching (our own daemon over a local socket;
the C surface already isolates everything behind handles). Real work; do not
start before S5 proves the multi-workspace life is the daily driver.

**S9 — Automations + MCP control.** Superset's scheduled agent runs and
`superset-mcp`: RUUAH exposes its sessions as an MCP server (spawn, send,
read block output) so an agent can drive terminals programmatically, plus
cron-shaped scheduled runs. Overlaps Orel's existing agent infra — scope it
with him when S5/S6 are live.

## Deliberately NOT taken

- **Warp's cloud** (Drive, Oz, session sharing, teams) — closed, server-side,
  and against the local-first grain of this machine.
- **Warp's AI-in-terminal UI** (agent conversation panes) — RUUAH's model is
  the opposite: the agent (Claude Code) runs *in* the terminal as a child, and
  the terminal serves it well (kitty keyboard P1.7, mouse P1.5, notifications).
  Revisit only if real use demands more.
- **A built-in code editor / LSP** (Warp's ADE direction) — RUUAH is a
  terminal; S6's diff panel is the boundary of "viewing code" here.

## Protocol dependencies (live in BACKLOG-2026.md)

Scrollback viewport (P1.6) unlocks S2 block navigation and S7 panes; kitty
keyboard (P1.7) + SGR mouse (P1.5) are what make CLI agents first-class; color
emoji (P0.2) is still the top visible gap in daily Claude-in-RUUAH use.
