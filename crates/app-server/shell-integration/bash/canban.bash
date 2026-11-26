# Canban shell integration for bash (launched via `bash --init-file`).
if [[ -r /etc/profile ]]; then
  source /etc/profile
fi
if [[ -r "$HOME/.bash_profile" ]]; then
  source "$HOME/.bash_profile"
elif [[ -r "$HOME/.bash_login" ]]; then
  source "$HOME/.bash_login"
elif [[ -r "$HOME/.profile" ]]; then
  source "$HOME/.profile"
fi

if [[ $- == *i* ]] && [[ -z "${__canban_hooks_loaded:-}" ]] && (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
  __canban_hooks_loaded=1
  __canban_cmd_active=""

  __canban_osc() {
    printf '\e]7501;id=canban-shell:%s\e\\' "$1"
  }

  __canban_b64() {
    printf '%s' "$1" | command base64 | command tr -d '\r\n'
  }

  __canban_ps0() {
    __canban_osc "state=working:app=shell"
  }

  __canban_precmd() {
    local code=$?
    if [[ -n "$__canban_cmd_active" ]]; then
      __canban_cmd_active=""
      local code_b64
      code_b64="$(__canban_b64 "$code")"
      if (( code == 0 )); then
        __canban_osc "state=done:app=shell:msg=${code_b64}"
      else
        __canban_osc "state=error:app=shell:msg=${code_b64}"
      fi
    fi
    return "$code"
  }

  PS0='${__canban_x[__canban_cmd_active=1]:-}$(__canban_ps0)'"${PS0:-}"
  if declare -p PROMPT_COMMAND >/dev/null 2>&1 && [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=("__canban_precmd" "${PROMPT_COMMAND[@]}")
  elif [[ -n "${PROMPT_COMMAND:-}" ]]; then
    PROMPT_COMMAND="__canban_precmd; $PROMPT_COMMAND"
  else
    PROMPT_COMMAND="__canban_precmd"
  fi
fi
