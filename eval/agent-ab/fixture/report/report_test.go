package report

// INTEGRATION CONTRACT — this file pins user-visible behavior.

import (
	"testing"

	"example.com/ledger/store"
)

func TestSummaryContract(t *testing.T) {
	l := store.New()
	l.Add(200)
	l.Add(300)
	l.Add(100)
	l.Add(400)

	want := "1. $2.00\n2. $3.00\n3. $1.00\ntotal: $10.00\n"
	if got := Summary(l); got != want {
		t.Fatalf("Summary() =\n%q\nwant\n%q", got, want)
	}
}
