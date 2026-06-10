package store

// Ledger records monetary amounts.
type Ledger struct {
	amounts []int
}

func New() *Ledger { return &Ledger{} }

// Add records an amount in cents. Negative amounts are refunds.
func (l *Ledger) Add(cents int) {
	l.amounts = append(l.amounts, cents)
}

// Amounts returns the recorded amounts in cents, in insertion order.
func (l *Ledger) Amounts() []int {
	out := make([]int, len(l.amounts))
	copy(out, l.amounts)
	return out
}

// Entries returns the recorded amounts in cents, in insertion order.
func (l *Ledger) Entries() []int {
	out := make([]int, len(l.amounts))
	copy(out, l.amounts)
	return out
}

// Total returns the sum of all recorded amounts in cents.
func (l *Ledger) Total() int {
	sum := 0
	for _, a := range l.amounts {
		sum += a
	}
	return sum
}
