package money

import "testing"

func TestFormat(t *testing.T) {
	cases := map[int]string{150: "$1.50", 0: "$0.00", -100: "-$1.00", 1000: "$10.00"}
	for cents, want := range cases {
		if got := Format(cents); got != want {
			t.Fatalf("Format(%d) = %q, want %q", cents, got, want)
		}
	}
}
