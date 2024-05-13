defmodule Fibonacci do
  def fib_cpu(0), do: 0
  def fib_cpu(1), do: 1
  def fib_cpu(n), do: fib(n - 1) + fib(n - 2)

  def fib(n) when n > 30 do
    parent = self()
    spawn(fn -> send(parent, fib(n - 1)) end)
    x = fib(n - 2)

    receive do
      y -> x + y
    end
  end

  def fib(n), do: Fibonacci.fib_cpu(n)
end

n = 52
IO.puts("fib(#{n}) = #{Fibonacci.fib(n)}")
