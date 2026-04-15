import gleam/erlang/process
import gleam/int
import gleam/io

fn fib_cpu(n: Int) -> Int {
  case n {
    0 -> 0
    1 -> 1
    n -> fib_cpu(n - 1) + fib_cpu(n - 2)
  }
}

fn fib(n: Int) -> Int {
  case n > 30 {
    True -> {
      let reply = process.new_subject()
      process.spawn(fn() { process.send(reply, fib(n - 1)) })
      fib(n - 2) + { process.receive_forever(reply) }
    }
    False -> fib_cpu(n)
  }
}

pub fn main() {
  let n = 52
  io.println("fib(" <> int.to_string(n) <> ") = " <> int.to_string(fib(n)))
}
