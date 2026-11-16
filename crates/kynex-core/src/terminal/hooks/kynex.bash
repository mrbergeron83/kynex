# Kynex's hooks: tell the terminal when a command starts and ends (and its exit code), and link
# what it prints, so `knx pane run --wait` can answer. Read with `bash --init-file` in place of
# the usual files, which it reads first. Nothing is printed.
if [ -n "${KYNEX_LOGIN-}" ]; then
    unset KYNEX_LOGIN
    [ -r /etc/profile ] && . /etc/profile
    if [ -r ~/.bash_profile ]; then
        . ~/.bash_profile
    elif [ -r ~/.bash_login ]; then
        . ~/.bash_login
    elif [ -r ~/.profile ]; then
        . ~/.profile
    fi
elif [ -r ~/.bashrc ]; then
    . ~/.bashrc
fi

if [ -z "${__kynex_p-}" ]; then
    __kynex_p=0
    __kynex_n=0
    __kynex_x=()
    __kynex_prompt() {
        local code=$?
        __kynex_p=$((__kynex_p + 1))
        printf '\033]8;;\033\\\033]0;knx-prompt:%s:%s:%s\007' "$__kynex_p" "$__kynex_n" "$code"
        return $code
    }
    # First, to see the command's exit code; it hands the code on to the rest.
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__kynex_prompt "${PROMPT_COMMAND[@]}")
    else
        PROMPT_COMMAND="__kynex_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
    fi
    # bash 4.4 and later show PS0 as a command starts; the subscript counts it (prompt
    # strings are expanded in the shell itself, so the count stays).
    PS0='${__kynex_x[__kynex_n+=1]}\e]8;;knx:out:${__kynex_n}\e\\\e]0;knx-exec:${__kynex_n}\a'"${PS0-}"
fi
