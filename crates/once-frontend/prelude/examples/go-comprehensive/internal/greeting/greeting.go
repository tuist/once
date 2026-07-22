package greeting

import "github.com/pkg/errors"

func Message(name string) (string, error) {
	if name == "" {
		return "", errors.New("name must not be empty")
	}
	return "Hello, " + name + Punctuation(), nil
}
