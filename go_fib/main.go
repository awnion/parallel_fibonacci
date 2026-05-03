package main

import (
	"fmt"
	"os"
	"strconv"
)

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
	n := uint64(52)
	if len(os.Args) > 1 {
		if v, err := strconv.ParseUint(os.Args[1], 10, 64); err == nil {
			n = v
		}
	}
	fmt.Printf("Fib %v = %v\n", n, fib(n))
}
