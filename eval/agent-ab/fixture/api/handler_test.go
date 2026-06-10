package api

// INTEGRATION CONTRACT — this file pins user-visible behavior.

import (
	"testing"

	"example.com/ledger/store"
)

func TestBalanceContract(t *testing.T) {
	l := store.New()
	Deposit(l, 500)
	Refund(l, 200)

	if got := Balance(l); got != "balance: $3.00" {
		t.Fatalf("Balance() = %q, want %q", got, "balance: $3.00")
	}
}
