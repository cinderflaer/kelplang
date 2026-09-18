impl "inner" with caps(fs = ["/etc/**"])

fun probe().R: bool = inner.check()
