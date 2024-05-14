import gleam/erlang/process
import gleam/int
import gleam/io

pub fn fib_cpu(n: Int) -> Int {
  case n {
    0 -> 0
    1 -> 1
    n -> fib_cpu(n - 1) + fib_cpu(n - 2)
  }
}

pub fn fib(n: Int) -> Int {
  case n {
    _ if n > 30 -> {
      let subject = process.new_subject()

      process.start(fn() { process.send(subject, fib(n - 1)) }, False)

      let y: Int = fib(n - 2)

      case process.receive(subject, 999_999) {
        Ok(x) -> x + y
        Error(_) -> panic as "something broke :("
      }
    }
    n -> fib_cpu(n)
  }
}

pub fn main() {
  let n: Int = 45
  io.println("fib(" <> int.to_string(n) <> ") = " <> int.to_string(fib_cpu(n)))
}
