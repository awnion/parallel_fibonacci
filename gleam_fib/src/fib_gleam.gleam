import gleam/int
import gleam/io
import gleam/otp/task

fn fib_cpu(n: Int) -> Int {
  case n {
    0 -> 0
    1 -> 1
    n -> fib_cpu(n - 1) + fib_cpu(n - 2)
  }
}

fn fib(n: Int) -> Int {
  case n {
    _ if n > 30 -> {
      let t = task.async(fn() { fib(n - 1) })
      fib(n - 2) + task.await_forever(t)
    }
    n -> fib_cpu(n)
  }
}

pub fn main() {
  let n = 52
  io.println("fib(" <> int.to_string(n) <> ") = " <> int.to_string(fib(n)))
}
