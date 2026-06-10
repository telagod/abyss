package money

import "fmt"

// Format renders an amount in cents as a dollar string, e.g. 150 → "$1.50".
func Format(cents int) string {
	sign := ""
	if cents < 0 {
		sign = "-"
		cents = -cents
	}
	return fmt.Sprintf("%s$%d.%02d", sign, cents/100, cents%100)
}
