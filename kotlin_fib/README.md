# Kotlin Fibonacci

Kotlin/JVM implementation of parallel Fibonacci numbers using Java virtual threads.

Requires Java 21+ for virtual threads.

## Run

```bash
cd kotlin_fib
kotlinc Main.kt -include-runtime -d fib.jar
kotlin fib.jar 52
```

Or use the wrapper:

```bash
./run.sh 52
```
