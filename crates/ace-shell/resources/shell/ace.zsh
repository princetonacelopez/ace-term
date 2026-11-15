# Ace Term Zsh Integration (non-destructive ZDOTDIR wrapper)
if [[ -n "${ACE_TERM_ORIG_ZDOTDIR:-}" ]]; then
    export ZDOTDIR="$ACE_TERM_ORIG_ZDOTDIR"
else
    unset ZDOTDIR
fi

_ace_user_zdotdir="${ZDOTDIR:-$HOME}"
[[ -f "$_ace_user_zdotdir/.zshenv" ]] && source "$_ace_user_zdotdir/.zshenv"
[[ -f "$_ace_user_zdotdir/.zprofile" ]] && source "$_ace_user_zdotdir/.zprofile"
[[ -f "$_ace_user_zdotdir/.zshrc" ]] && source "$_ace_user_zdotdir/.zshrc"
[[ -f "$_ace_user_zdotdir/.zlogin" ]] && source "$_ace_user_zdotdir/.zlogin"

autoload -Uz add-zsh-hook

__ace_osc7() {
    printf '\e]7;file://%s%s\a' "${HOST:-localhost}" "$PWD"
}

__ace_precmd() {
    local ec=$?
    printf '\e]133;D;%s\a' "$ec"
    __ace_osc7
    printf '\e]133;A\a'
}

__ace_preexec() {
    printf '\e]133;C\a'
}

add-zsh-hook precmd __ace_precmd
add-zsh-hook preexec __ace_preexec
__ace_osc7
printf '\e]133;A\a'
