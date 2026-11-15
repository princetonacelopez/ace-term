# Ace Term Bash Integration (non-destructive init)
[[ -f /etc/profile ]] && source /etc/profile
if [[ -f "$HOME/.bash_profile" ]]; then
    source "$HOME/.bash_profile"
elif [[ -f "$HOME/.bashrc" ]]; then
    source "$HOME/.bashrc"
fi

__ace_osc7() {
    printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "$PWD"
}

__ace_prompt_command() {
    local ec=$?
    printf '\e]133;D;%s\a' "$ec"
    __ace_osc7
    printf '\e]133;A\a'
}

if [[ -z "${PROMPT_COMMAND:-}" ]]; then
    PROMPT_COMMAND="__ace_prompt_command"
else
    PROMPT_COMMAND="__ace_prompt_command;${PROMPT_COMMAND}"
fi
__ace_osc7
printf '\e]133;A\a'
