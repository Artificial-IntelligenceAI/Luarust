package main

import "fmt"

func main() {
	var sum uint64 = 0
	for i := uint64(1); i <= ITERATIONS; i++ {
		sum = (sum + i) % 1000000007
	}
	fmt.Println(sum)
}
