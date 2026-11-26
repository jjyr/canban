# Canban shell integration for zsh.
# Restore user ZDOTDIR before sourcing startup files, then attach preexec/precmd hooks
# for interactive shells to report foreground command status via namespaced OSC 7501.
if [[ -n ${CANBAN_USER_ZDOTDIR+x} ]]; then
  export ZDOTDIR="$CANBAN_USER_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset CANBAN_USER_ZDOTDIR

if [[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
  builtin source "${ZDOTDIR:-$HOME}/.zshenv"
fi

if [[ -o interactive ]] && [[ -z ${__canban_hooks_loaded+x} ]]; then
  typeset -g __canban_hooks_loaded=1
  typeset -g __canban_cmd_active=""

  __canban_osc() {
    printf '\e]7501;id=canban-shell:%s\e\\' "$1"
  }

  __canban_b64() {
    printf '%s' "$1" | command base64 | command tr -d '\r\n'
  }

  __canban_preexec() {
    __canban_cmd_active=1
    local raw="${1:-}"
    local -a words
    words=(${(z)raw})
    while (( ${#words} > 1 )) && [[ "${words[1]}" == [A-Za-z_][A-Za-z0-9_]*=* ]]; do
      shift words
    done
    local prog="${words[1]:t}"
    prog="${prog[1,32]}"
    if [[ -n "$prog" ]]; then
      local title_b64
      title_b64="$(__canban_b64 "$prog")"
      __canban_osc "state=working:app=shell:title=${title_b64}"
    else
      __canban_osc "state=working:app=shell"
    fi
  }

  __canban_precmd() {
    local code=$?
    [[ -n "$__canban_cmd_active" ]] || return 0
    __canban_cmd_active=""
    local code_b64
    code_b64="$(__canban_b64 "$code")"
    if (( code == 0 )); then
      __canban_osc "state=done:app=shell:msg=${code_b64}"
    else
      __canban_osc "state=error:app=shell:msg=${code_b64}"
    fi
  }

  autoload -Uz add-zsh-hook
  add-zsh-hook preexec __canban_preexec
  add-zsh-hook precmd __canban_precmd
fi
