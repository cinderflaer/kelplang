// M5.3 rejection: a cycle. Neither alias is used, so this is exactly the
// eager table check — the same rule imports follow (an import cycle is an
// error even if nothing calls into it).
alias %*a*% = %*b*%
alias %*b*% = %*a*%

fun main() {
    print("unreachable")
}
