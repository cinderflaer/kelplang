pd val app_name = "shapes"
pd var hit_count = 0

fun label().R: &str = app_name

fun banner() {
    print("[$app_name] hits = $hit_count")
}

fun unrelated(n: i32).R: i32 = n * 2

fun main() {
    print(app_name)
    print(label())
    banner()
    print(unrelated(21))
}
