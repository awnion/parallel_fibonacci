package main

import "fmt"

func fib_cpu(n uint64) uint64 {
	switch n {
	case 0:
		return 0
	case 1:
		return 1
	default:
		return fib_cpu(n-1) + fib_cpu(n-2)
	}
}

func fib(n uint64) uint64 {
	if n > 30 {
		ch := make(chan uint64, 2)
		go func() { ch <- fib(n - 1) }()

		r := fib(n - 2)
		l := <-ch
		return l + r
	} else {
		return fib_cpu(n)
	}
}

func main() {
	const n = 52
	fmt.Printf("Fib %v = %v\n", n, fib(n))
}
