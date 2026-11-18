package main

import (
	"fmt"
	"log"

	"example.com/struo-go-comprehensive/internal/greeting"
)

func main() {
	message, err := greeting.Message("Struo")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(message)
}
