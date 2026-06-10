package util

import "testing"

func TestClampInclusive(t *testing.T) {
	if got := Clamp(5, 0, 3); got != 3 {
		t.Fatalf("Clamp(5,0,3) = %d, want 3", got)
	}
	if got := Clamp(3, 0, 3); got != 3 {
		t.Fatalf("Clamp(3,0,3) = %d, want 3", got)
	}
	if got := Clamp(-1, 0, 3); got != 0 {
		t.Fatalf("Clamp(-1,0,3) = %d, want 0", got)
	}
}
