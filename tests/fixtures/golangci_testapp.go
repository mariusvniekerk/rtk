package main

import (
	"fmt"
	"os"
)

// Intentional lint issues:
// 1. unused variable (ineffassign)
// 2. unchecked error (errcheck)
// 3. unused import — fmt used, os used partially

func main() {
	x := 42    // ineffassign: x is overwritten before use
	x = 100
	fmt.Println(x)

	f, _ := os.Open("nonexistent.txt") // errcheck: error ignored
	fmt.Println(f)

	doStuff()
}

func doStuff() {
	var err error
	_ = err
	os.Getwd() // errcheck: return value ignored
}
