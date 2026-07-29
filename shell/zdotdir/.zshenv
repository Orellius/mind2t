# RUUAH VT's ZDOTDIR bootstrap.
#
# The app points ZDOTDIR here for the shells it spawns, so this file runs FIRST, loads
# the integration hooks, and immediately hands control back to the user's real zsh
# startup chain. zsh resolves each startup file when it reaches it, so restoring ZDOTDIR
# before sourcing the user's .zshenv makes .zprofile/.zshrc/.zlogin load from the user's
# own directory exactly as they would without RUUAH in the picture.
#
# The hooks survive the user's .zshrc because they are registered via add-zsh-hook,
# which appends -- a user overwriting precmd_functions wholesale opts out, and that is
# their call.

[[ -n "$RUUAH_INTEGRATION" && -f "$RUUAH_INTEGRATION" ]] && builtin source "$RUUAH_INTEGRATION"

if [[ -n "$RUUAH_USER_ZDOTDIR" ]]; then
    ZDOTDIR="$RUUAH_USER_ZDOTDIR"
    unset RUUAH_USER_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && builtin source "${ZDOTDIR:-$HOME}/.zshenv"
