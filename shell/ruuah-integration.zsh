# RUUAH shell integration: OSC 133 semantic marks (FinalTerm / semantic-prompt scheme).
#
# Three marks are enough for blocks:
#   A  before the prompt renders     (precmd)      -> prompt cells
#   B  after the prompt, before input (end of PS1) -> input cells
#   C  before command output          (preexec)    -> output cells
#
# Sourced by the bundled ZDOTDIR bootstrap (see zdotdir/.zshenv) inside RUUAH VT windows
# only -- it keys on nothing global and leaves non-RUUAH shells untouched. Idempotent:
# sourcing twice registers nothing twice.

[[ -n "$_RUUAH_INTEGRATION_LOADED" ]] && return
_RUUAH_INTEGRATION_LOADED=1

autoload -Uz add-zsh-hook

_ruuah_precmd() {
    # A: the prompt starts here. B rides the end of PS1 so everything the user types
    # after it is input-marked; %{...%} tells zsh the escape occupies no columns.
    [[ "$PROMPT" == *']133;B'* ]] || PROMPT="${PROMPT}%{$(printf '\033]133;B\007')%}"
    printf '\033]133;A\007'
}

_ruuah_preexec() {
    # C: the command is about to run; everything from here is its output.
    printf '\033]133;C\007'
}

add-zsh-hook precmd _ruuah_precmd
add-zsh-hook preexec _ruuah_preexec
