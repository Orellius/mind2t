# RUUAH shell integration: OSC 133 semantic marks (FinalTerm / semantic-prompt scheme).
#
# Four marks drive blocks:
#   A  prompt starts        (rides the front of PS1)
#   B  input starts         (rides the end of PS1)
#   C  output starts        (preexec)
#   D  command finished     (next precmd, with the exit status when a command ran)
#
# The naive shape -- append B to PROMPT from a hook registered at .zshenv time -- dies
# under any theme whose own precmd regenerates PROMPT after ours: registration order is
# load order, .zshenv loads before .zshrc, so the theme always wins and the input mark
# silently disappears (measured live under starship's transient-prompt restore,
# 2026-07-30; pinned by crates/host/tests/shell_integration.rs). The shape that survives
# is the one the oracle's own integration uses (../ruuah/src/shell-integration/zsh, MIT):
#
#   1. defer real registration to the FIRST precmd, which runs after the user's rc;
#   2. keep our precmd LAST in precmd_functions, re-sorting every cycle it is displaced;
#   3. re-mark whatever PS1 the theme just produced instead of fighting it, restoring
#      the saved clean copy first so marks never stack.
#
# Not covered yet: PS2 continuation marks, and the zle-invoked-precmd edge Ghostty's
# cl=line option exists for. Both are documented gaps, not oversights.

[[ -o interactive ]] || return
[[ -n "$_RUUAH_INTEGRATION_LOADED" ]] && return
_RUUAH_INTEGRATION_LOADED=1

builtin typeset -ag precmd_functions
precmd_functions+=(_ruuah_deferred_init)

_ruuah_deferred_init() {
    builtin emulate -L zsh
    autoload -Uz add-zsh-hook
    precmd_functions=(${precmd_functions:#_ruuah_deferred_init})
    add-zsh-hook precmd _ruuah_precmd
    add-zsh-hook preexec _ruuah_preexec
    # Mark this first prompt too; a theme's own precmd may still rewrite PS1 after us
    # this one cycle, which the re-sort corrects from the second prompt on.
    _ruuah_precmd
}

typeset -g _ruuah_report_exit=0
typeset -g _ruuah_saved_ps1=
typeset -g _ruuah_marked_ps1=

_ruuah_precmd() {
    builtin local ret=$?
    builtin emulate -L zsh

    # D closes the previous command's output region -- with the exit status when a
    # command actually ran (C was emitted), bare when the line was empty.
    if (( _ruuah_report_exit )); then
        builtin print -n "\e]133;D;${ret}\a"
        _ruuah_report_exit=0
    fi

    # Stay last: whoever runs last owns the final PROMPT. The re-sort takes effect the
    # NEXT cycle, which is why a freshly registered theme steals at most one prompt.
    if [[ "${precmd_functions[-1]}" != _ruuah_precmd ]]; then
        precmd_functions=(${precmd_functions:#_ruuah_precmd} _ruuah_precmd)
    fi

    # Re-mark from the PS1 the theme just produced. If PS1 still wears our marks from
    # last cycle (nothing touched it), strip back to the saved clean copy first.
    if [[ -n "$_ruuah_marked_ps1" && "$PS1" == "$_ruuah_marked_ps1" ]]; then
        PS1="$_ruuah_saved_ps1"
    fi
    _ruuah_saved_ps1="$PS1"
    # A bare trailing % would swallow the %{ of the B mark.
    [[ "$PS1" == *[^%]% || "$PS1" == % ]] && PS1="${PS1}%"
    PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
    _ruuah_marked_ps1="$PS1"
}

_ruuah_preexec() {
    _ruuah_report_exit=1
    builtin print -n "\e]133;C\a"
}
