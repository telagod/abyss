package ids

import "testing"

func TestNextStartsAtOne(t *testing.T) {
	var c Counter
	if got := c.Next(); got != 1 {
		t.Fatalf("first Next() = %d, want 1", got)
	}
	if got := c.Next(); got != 2 {
		t.Fatalf("second Next() = %d, want 2", got)
	}
}
