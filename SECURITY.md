# Security Policy

## Reporting a vulnerability

Report privately through GitHub: **[open a security advisory](https://github.com/Orellius/mind2t/security/advisories/new)**.
Please do not open a public issue for anything exploitable.

Include the byte sequence or reproduction steps, the version or commit, and what you believe
the impact is. A proof of concept that makes the defect fire is worth more than a description
of it, and this project's whole testing culture is built on that preference.

Expect a first response within seven days. This is a small project with one maintainer, so
that is a realistic figure rather than an aspirational one.

There is no bounty programme.

## Supported versions

Only the latest release and `main` receive fixes. The project is pre-1.0 and there are no
backports.

## Why a terminal emulator is a security surface

A terminal renders bytes chosen by whatever program is running in it, and those bytes are
often not chosen by you: the output of `cat` on a downloaded file, a compiler diagnostic
quoting a hostile identifier, an SSH session to a machine you do not control, or a coding
agent printing something it read off the internet. Escape sequences in that stream are
instructions to the terminal, not text.

The interesting question for any terminal is therefore what a remote byte stream is allowed
to make it do. The current answers:

**Screen inspection is off by default.** DECRQCRA (per-cell checksum) and WINOPS 18 (size
report) let a program read back what is on your screen, which in a shared or piped session can
include content the program was never shown. They live behind a `reports` grant in
`~/.mind2t/config.toml`, off unless you turn it on, and it is an embedder grant that RIS
cannot revoke. Enabling it is a deliberate act with a stated cost.

**The clipboard can be written, never read.** OSC 52 set-clipboard is implemented. OSC 52
read-clipboard is not, and that is a decision rather than an omission: answering it means
handing an arbitrary program the contents of your clipboard.

**Paste is bracketed.** Mode 2004 fencing is implemented with the reference-measured
encoding, so a shell that opts in can tell pasted text from typed text and refuse to run a
newline it did not see you press.

**Hyperlinks require a modifier.** OSC 8 link stamps are stored and survive scroll and
resize, but nothing opens on hover. Opening is cmd+click, an explicit gesture.

**Auto-approve bypasses are refused, not stripped.** The agent launcher will not spawn an
agent CLI carrying `--yolo`, `--dangerously-skip-permissions` and the rest. It refuses the
launch rather than removing the flag and proceeding, because stripping would leave you
believing approvals were on when they were not. Matching is on whole argv tokens, so a guard
that also refuses harmless near-misses does not get routed around.

**No terminal bytes reach the browser engine.** The chrome strip is a WKWebView drawing
documents, loaded from `file://` with a navigation policy that refuses everything else, and
it is a single self-contained document that resolves no subresources. Terminal pixels,
keystrokes and frames never cross into it. Panels are off unless enabled in config.

**Decoders run on untrusted input.** The kitty graphics and sixel paths decode image data
that arrives from the child process, and PNG decoding is a real memory-safety surface even in
Rust. Denial of service through a pathological image, an unbounded escape sequence or a
synchronized-output stall is in scope; the synchronized-output path already carries an
anti-stuck budget so a wedged frame cannot freeze the display permanently.

**Unsafe code is concentrated and has an oracle.** The pty crate owns the process's only I/O
and its only pty `unsafe` block. The ABI crates hand raw handles across a C boundary, where a
native test run passing proves nothing about undefined behaviour. Miri is the gate for that
class:

```sh
cargo +nightly miri test -p mind2t-vt-abi --test soundness
```

Run it whenever the handle model changes. Two real defects in this repository were only ever
visible under Miri.

## In scope

- Escape sequences that cause memory unsafety, a crash, or an unbounded resource commitment.
- Any path by which a remote byte stream reads back screen, clipboard or environment content
  without the corresponding grant.
- Anything that executes a command without a human gesture, including through paste,
  hyperlinks, workflows or the agent launcher.
- Sandbox or navigation-policy escapes in the panel webview.
- Guards that can be shown not to fire.

## Out of scope

- Behaviour that matches the reference implementation and is pinned as such in the corpus.
  Report it anyway if you think the reference is also wrong; it just gets handled as a
  correctness question rather than as a vulnerability.
- Anything that requires the `reports` grant to already be enabled, unless it exceeds what
  that grant documents.
- Compromise of the machine the terminal runs on. A terminal cannot defend against a program
  that already runs as you.
- Third-party dependency advisories with no reachable path in this code. Please still say so,
  with the path you looked for.
