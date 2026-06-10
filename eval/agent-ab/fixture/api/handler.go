package api

import (
	"example.com/ledger/money"
	"example.com/ledger/store"
)

// Deposit records a positive amount in cents.
func Deposit(l *store.Ledger, cents int) {
	l.Add(cents)
}

// Refund records a refund of the given amount in cents.
func Refund(l *store.Ledger, cents int) {
	l.Add(-cents)
}

// Balance renders the current balance, e.g. "balance: $3.00".
func Balance(l *store.Ledger) string {
	return "balance: " + money.Format(l.Total())
}
