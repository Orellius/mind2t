# Mind2t shell integration: OSC 133 semantic marks (FinalTerm / semantic-prompt scheme).
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
# This file also emits OSC 7 (the working directory), because NOTHING ELSE DOES in our
# windows: macOS defines update_terminal_cwd in /etc/zshrc_Apple_Terminal and /etc/zshrc
# sources that file only when $TERM_PROGRAM is Apple_Terminal, which we never set
# (verified 2026-07-31). The host keys command history by it, so a shell without this
# integration simply gets the global history it always had.
#
# Not covered yet: PS2 continuation marks, and the zle-invoked-precmd edge Ghostty's
# cl=line option exists for. Both are documented gaps, not oversights.

[[ -o interactive ]] || return
[[ -n "$_MIND2T_INTEGRATION_LOADED" ]] && return
_MIND2T_INTEGRATION_LOADED=1

builtin typeset -ag precmd_functions
precmd_functions+=(_mind2t_deferred_init)

_mind2t_deferred_init() {
    builtin emulate -L zsh
    autoload -Uz add-zsh-hook
    precmd_functions=(${precmd_functions:#_mind2t_deferred_init})
    add-zsh-hook precmd _mind2t_precmd
    add-zsh-hook preexec _mind2t_preexec
    # Mark this first prompt too; a theme's own precmd may still rewrite PS1 after us
    # this one cycle, which the re-sort corrects from the second prompt on.
    _mind2t_precmd
}

typeset -g _mind2t_report_exit=0
typeset -g _mind2t_saved_ps1=
typeset -g _mind2t_marked_ps1=

_mind2t_precmd() {
    builtin local ret=$?
    builtin emulate -L zsh

    # D closes the previous command's output region -- with the exit status when a
    # command actually ran (C was emitted), bare when the line was empty.
    if (( _mind2t_report_exit )); then
        builtin print -n "\e]133;D;${ret}\a"
        _mind2t_report_exit=0
    fi

    # Stay last: whoever runs last owns the final PROMPT. The re-sort takes effect the
    # NEXT cycle, which is why a freshly registered theme steals at most one prompt.
    if [[ "${precmd_functions[-1]}" != _mind2t_precmd ]]; then
        precmd_functions=(${precmd_functions:#_mind2t_precmd} _mind2t_precmd)
    fi

    # Every prompt, not only on chpwd: a command may cd without the hook (a subshell that
    # exits, `popd` inside a function), and one report per prompt is cheap.
    _mind2t_report_pwd

    # Re-mark from the PS1 the theme just produced. If PS1 still wears our marks from
    # last cycle (nothing touched it), strip back to the saved clean copy first.
    if [[ -n "$_mind2t_marked_ps1" && "$PS1" == "$_mind2t_marked_ps1" ]]; then
        PS1="$_mind2t_saved_ps1"
    fi
    _mind2t_saved_ps1="$PS1"
    # A bare trailing % would swallow the %{ of the B mark.
    [[ "$PS1" == *[^%]% || "$PS1" == % ]] && PS1="${PS1}%"
    PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
    _mind2t_marked_ps1="$PS1"
}

_mind2t_preexec() {
    _mind2t_report_exit=1
    builtin print -n "\e]133;C\a"
}

# OSC 7: report the working directory as a file:// URI.
#
# The path is percent-encoded because a directory may contain spaces, quotes, or any byte
# that is not a slash -- an unencoded report of "/Users/orel/My Code" is a malformed URI,
# and the receiving side cannot tell where the path ended. zsh's own ${var//pattern/repl}
# cannot do this per byte, so the encoding walks the string; paths are short and this runs
# once per directory change, not once per keystroke.
_mind2t_report_pwd() {
    # NOT named `path`: in zsh that identifier is tied to $PATH as an array, so
    # ${path[i]} indexes PATH entries and ${#path} counts them. Measured -- the first
    # version of this function reported "file://host%2F" for every directory.
    builtin local cwd="$PWD" encoded="" i char
    for (( i = 1; i <= ${#cwd}; i++ )); do
        char="${cwd[i]}"
        case "$char" in
            # RFC 3986 unreserved, plus the separator itself.
            [A-Za-z0-9._~/-]) encoded+="$char" ;;
            *) encoded+="$(builtin printf '%%%02X' "'$char")" ;;
        esac
    done
    builtin print -n "\e]7;file://${HOST}${encoded}\a"
}
