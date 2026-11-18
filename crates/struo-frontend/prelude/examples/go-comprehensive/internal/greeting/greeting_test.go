package greeting

import "testing"

func TestMessage(t *testing.T) {
	message, err := Message("Struo")
	if err != nil {
		t.Fatal(err)
	}
	if message != "Hello, Struo!" {
		t.Fatalf("unexpected message: %q", message)
	}
}

func TestEmptyName(t *testing.T) {
	if _, err := Message(""); err == nil {
		t.Fatal("expected an error")
	}
}

func BenchmarkMessage(b *testing.B) {
	for range b.N {
		_, _ = Message("Struo")
	}
}
