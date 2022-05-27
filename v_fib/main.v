fn fib_cpu(n u64) u64 {
	match n {
		0 {
			return 0
		}
		1 {
			return 1
		}
		else {
			return fib_cpu(n - 1) + fib_cpu(n - 2)
		}
	}
}

fn fib(n u64) u64 {
	if n > 30 {
		tr := go fib(n - 1)
		r := fib(n - 2)
		l := tr.wait()
		return l + r
	} else {
		return fib_cpu(n)
	}
}

fn main() {
	n := u64(52)
	println('Fib $n = ${fib(n)}')
}
