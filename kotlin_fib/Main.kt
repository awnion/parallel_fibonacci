import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

private const val Threshold = 30UL
private const val DefaultN = 52UL

private fun fibCpu(n: ULong): ULong =
    when (n) {
        0UL -> 0UL
        1UL -> 1UL
        else -> fibCpu(n - 1UL) + fibCpu(n - 2UL)
    }

private fun fib(n: ULong, executor: ExecutorService): ULong {
    if (n > Threshold) {
        val left = executor.submit<ULong> { fib(n - 1UL, executor) }
        val right = fib(n - 2UL, executor)

        return left.get() + right
    }

    return fibCpu(n)
}

private fun fib(n: ULong): ULong {
    val executor = Executors.newVirtualThreadPerTaskExecutor()

    try {
        return fib(n, executor)
    } finally {
        executor.shutdown()
    }
}

fun main(args: Array<String>) {
    val n = args.getOrNull(0)?.toULongOrNull() ?: DefaultN
    println("fib($n) = ${fib(n)}")
}
