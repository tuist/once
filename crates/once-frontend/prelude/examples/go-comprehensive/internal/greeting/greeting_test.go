package greeting

import "testing"

func TestMessage(t *testing.T) {
	message, err := Message("Once")
	if err != nil {
		t.Fatal(err)
	}
	if message != "Hello, Once!" {
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
		_, _ = Message("Once")
	}
}
