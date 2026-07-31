# RUUAH app slices - the homegrown map from Warp OSS and Superset

> **2026-07-30 append-all wave:** S2 compat CLOSED (integration rewritten on the
> oracle's own zsh pattern -- deferred init, stay-last, re-mark the theme's PS1;
> proven against a starship-shaped hostile rc through the C surface; the operator's
> real .zshrc was the measured cause). The left sidebar is REPLACED by a top tab strip
> 1:1 to the operator's Warp reference (docs/images/top-tabs-20260730.png; avatars
> omitted -- no collaborators exist). Default window now 1120x700. App bundle is now
> properly ad-hoc signed (TCC can attribute it -- Desktop/Documents prompts work; an
> unsigned bundle was silently denied with no prompt). Config grew font-family,
> font-ligatures. REMAINING QUEUE: S3 cmd+K palette + workflows, S4 autosuggestions,
> S5+ as written below.


Written 2026-07-29 (IDT), researched live the same day. `BACKLOG-2026.md` is the
*protocol* backlog (what the VT layer owes any terminal); this file is the *app*
backlog - what makes RUUAH a product rather than a viewport. Sources, verified
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
   `warpdotdev/warp`** - AGPL v3 in a private repo is contamination, and ELv2
   is no better. We study *behavior* from docs and the product, then implement
   from our own architecture. The one importable artifact is **data**: Fig's
   completion *specs* (MIT) are declarative JSON we may consume with our own
   engine.
2. **THE IRON RULES HOLD.** Harness before slice; controls seen to fail; the
   core stays pure (no I/O); bidi stays in the renderer. App slices live in
   `crates/host` + `swift/` unless a real observable forces a core seam - and
   then the corpus pins it.

## What RUUAH already has that these products build on

- **OSC 133 semantic zones in the core** (`semantic.rs`, slice 5.6): prompt /
  input / output regions, corpus-pinned. This is the exact substrate Warp's
  signature Blocks feature needs - most terminals bolt it on; we start with it.
- **Session sidebar** (2026-07-29): one `RuuahHost` per session, background
  sessions keep running. This is the seed of Superset's workspace list.
- **Frame mode-bits + reported background**: the pattern for any state the GUI
  needs from the terminal without touching the pump thread.

## The slices, ordered

**S1 - Settings file + themes.** Warp's settings file + terminal appearance,
and Orel's stated want. A TOML at `~/.ruuah/config.toml`: font, size, theme,
keybinds, default shell. A theme = a palette file (16 ANSI + fg/bg/cursor);
`Palette` already resolves everything through `default_*` fields, and the
margin already follows the reported background - themes drop in. Shape:
host gains `ruuah_host_spawn` options for palette (or a set-palette call);
Swift reads the TOML. Oracle: a theme file with a distinct background must
recolor grid AND margin in a window capture; a bad file must fall back loudly.
*Prereq for S2+ (keybinds live in the settings).*

**S2 - Blocks v1. LANDED 2026-07-29, one compat gap open.** The C surface
carries per-row OSC 133 classes (`RuuahHostFrame.row_semantics`) and filtered
row text (`ruuah_host_row_text` - the input filter is what makes copy-command
return `ls -la` out of `$ ls -la`; both pinned in host_abi with a no-marks
control). Swift: gutter bars per block in the left margin, click pops copy
command / copy output / run again; `shell/ruuah-integration.zsh` +
`shell/zdotdir/.zshenv` bootstrap wired via ZDOTDIR env in the .app. Proven:
scripted two-command child shows two bars with the boundary at the block seam
(`docs/images/blocks-gutter-20260729.png`, pixel-scanned), and a PRISTINE zsh
through the real integration shows the live prompt bar.
**OPEN - real-config compat:** in Orel's own shell (starship transient prompt
+ iTerm2 shell integration) no rows classify. Suspects, measured 2026-07-29:
starship's precmd regenerates PROMPT after ours (wipes the B mark);
iTerm2's integration interleaves `133;C;`/`133;D;0` (marks WITH options) whose
interplay with our strict parser is unmeasured. Next round: instrument
(log unparsed OSC 133 forms), measure upstream's exact option tolerance, and
decide mark precedence. Until then blocks light up only without those
frameworks. *V2 later: sticky command header, block search, collapse.*

**S3 - Command palette + workflows.** cmd+K palette (Swift, pure UI): actions
(new session, switch, copy block, theme switch) + **workflows** = parameterized
command templates in YAML/TOML under `~/.ruuah/workflows/` (Warp's shape;
plain data, our format). Picking one fills the input with placeholders. Cheap,
high daily value. Oracle: unit-test the placeholder substitution; palette
exercised by hand (aesthetics = Orel).

**S4 - History autosuggestions. PHASE ONE DONE 2026-07-30** (`s4-autosuggest`).
Fish-style ghost text: `host/src/suggest.rs` (append-only store, 10k cap,
consecutive-dupe collapse, PROPER-prefix most-recent-wins matcher - the
deterministic fixture oracle, unit-tested both directions) behind 4 C exports
(`ruuah_history_*`, 49 total; C-surface round-trip test incl. persistence
across handles). History records when a NEW block appears below the last (the
S2 OSC 133 rails - no shell integration, no suggestions, the named
dependency). Ghost = dim CATextLayer at the caret (RuuahHostFrame grew
cursor_col/row/visible), shown only at the live bottom with the caret at
line end; bare right-arrow accepts by pasting the remainder. NOT keyed by
cwd yet; multiline commands refused;
Swift ghost visuals shipped [untested - needs your eyes] (window imaging
went dark mid-tap - environment, documented in the PR). Phase two
(Fig-spec completions) unstarted.

**DONE 2026-07-31 (`s4-cwd-history`): history is keyed by directory.** Entries
carry the directory they ran in, and a suggestion PREFERS a match made in the
current one, falling back to the newest match anywhere - fish's rule, and the
fallback is what stops the ghost vanishing the moment you `cd` somewhere new.
The shell integration now emits OSC 7 itself (nothing else does in our windows);
the host normalizes the URI, so the raw report crosses the C surface untouched
and exactly one place knows how to decode it. Old history files load unchanged:
a line with no tab is a command with no directory, which is the pre-cwd format.

**Original note, for the record:** OSC 7 is tracked, corpus-pinned against the real oracle, and
crosses the C surface as event kind 7 carrying the RAW report. What remains
for cwd-keyed history is entirely host-side and is its own slice: consume
kind 7 per session, percent-decode and strip the `file://host` prefix THERE
(the core must never decode - it stores what the child sent, and so does the
oracle), key the history store by the decoded path, and decide the fallback
when a directory has no history yet.

**The shell dependency, verified rather than assumed (2026-07-31): nothing
will emit OSC 7 into our windows today.** On macOS the emitter is
`update_terminal_cwd` in `/etc/zshrc_Apple_Terminal` (defined line 16, hooked
to `precmd` line 43), and `/etc/zshrc:74` sources that file only when
`$TERM_PROGRAM` is `Apple_Terminal`. We do not set `TERM_PROGRAM` at all, so
the hook never installs. `ruuah-integration.zsh` therefore has to emit OSC 7
itself, the same way it already emits the OSC 133 marks - which is good news
for the consumer slice, because it means the reported format is ours to fix
(`file://$HOST$PWD`, percent-encoded) rather than something to sniff.

**S5 - Worktree workspaces (the Superset/convoy shape).** "New workspace" =
create a git worktree of the current repo + spawn a session (optionally
launching a CLI agent - claude, codex) inside it. Sidebar groups sessions by
workspace; closing a workspace offers to remove the worktree. This is
**convoy's thesis** (tools/convoy, Orel's own prior art) - Superset shipping it
as a product validates the direction; RUUAH absorbing it is Orel's call on
convoy's future. Oracle: spawn → `git worktree list` shows it; close → gone;
two agents editing in parallel never touch each other's tree.

**S6 - Diff review panel.** Superset's dashboard, one workspace at a time:
changed files + unified diff, refreshed on demand (`git status`/`diff`
host-side, rendered Swift-side), "open in editor" per file. Pairs with S5.
Oracle: scripted mutation in a worktree must appear in the panel byte-exact.

**S7 - Splits and tabs: REFUSED for now (Orel, 2026-07-29).** The sidebar IS
the window management: each session is its own full terminal surface, switched
instantly from the list, closed with its row's X. One thing on screen at a
time is the model. Revisit only if real daily use produces the sentence "I
need these two side by side" - and then as a deliberate decision, not a drift.

**S8 - Persistent sessions.** Superset's "closing the laptop doesn't kill the
sessions". V1 exists today (sessions outlive their window inside the app).
V2 - sessions survive the app relaunching - means a detached host process
owning the ptys with the app reattaching (our own daemon over a local socket;
the C surface already isolates everything behind handles). Real work; do not
start before S5 proves the multi-workspace life is the daily driver.

**S9 - Automations + MCP control.** Superset's scheduled agent runs and
`superset-mcp`: RUUAH exposes its sessions as an MCP server (spawn, send,
read block output) so an agent can drive terminals programmatically, plus
cron-shaped scheduled runs. Overlaps Orel's existing agent infra - scope it
with him when S5/S6 are live.

## Deliberately NOT taken

- **Warp's cloud** (Drive, Oz, session sharing, teams) - closed, server-side,
  and against the local-first grain of this machine.
- **Warp's AI-in-terminal UI** (agent conversation panes) - RUUAH's model is
  the opposite: the agent (Claude Code) runs *in* the terminal as a child, and
  the terminal serves it well (kitty keyboard P1.7, mouse P1.5, notifications).
  Revisit only if real use demands more.
- **A built-in code editor / LSP** (Warp's ADE direction) - RUUAH is a
  terminal; S6's diff panel is the boundary of "viewing code" here.

## Protocol dependencies (live in BACKLOG-2026.md)

Scrollback viewport (P1.6) unlocks S2 block navigation and S7 panes; kitty
keyboard (P1.7) + SGR mouse (P1.5) are what make CLI agents first-class; color
emoji (P0.2) is still the top visible gap in daily Claude-in-RUUAH use.
