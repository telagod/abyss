package store

import "testing"

func TestAddAndTotal(t *testing.T) {
	l := New()
	l.Add(150)
	l.Add(-50)
	if got := l.Total(); got != 100 {
		t.Fatalf("Total() = %d, want 100", got)
	}
}

func TestAmountsInsertionOrder(t *testing.T) {
	l := New()
	l.Add(300)
	l.Add(100)
	got := l.Amounts()
	if got[0] != 300 || got[1] != 100 {
		t.Fatalf("Amounts() = %v, want [300 100]", got)
	}
}
