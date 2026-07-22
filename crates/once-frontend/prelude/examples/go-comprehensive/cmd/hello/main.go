package main

import (
	"fmt"
	"log"

	"example.com/once-go-comprehensive/internal/greeting"
)

func main() {
	message, err := greeting.Message("Once")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(message)
}
