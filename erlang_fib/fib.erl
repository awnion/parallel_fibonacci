-module(fib).
-export([fib/1]).

parallel_fib(N) ->
    Parent = self(),
    spawn(fun () -> Parent ! fib(N - 1) end),
    X = fib(N - 2),
    receive Y -> Y end,
    X + Y.

fib(N) when N > 20 -> parallel_fib(N);
fib(0) -> 0;
fib(1) -> 1;
fib(N) -> fib(N - 1) + fib(N - 2).