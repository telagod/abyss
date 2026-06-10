package report

import (
	"fmt"
	"strings"

	"example.com/ledger/ids"
	"example.com/ledger/money"
	"example.com/ledger/store"
	"example.com/ledger/util"
)

// Summary renders up to the first three recorded amounts as numbered lines,
// followed by the ledger total.
func Summary(l *store.Ledger) string {
	amounts := l.Amounts()
	shown := util.Clamp(len(amounts), 0, 3)

	var b strings.Builder
	var c ids.Counter
	for i := 0; i < shown; i++ {
		fmt.Fprintf(&b, "%d. %s\n", c.Next(), money.Format(amounts[i]))
	}
	fmt.Fprintf(&b, "total: %s\n", money.Format(l.Total()))
	return b.String()
}
