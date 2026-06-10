package ids

// Counter hands out sequential identifiers.
type Counter struct {
	n int
}

// Next returns the next identifier. The first call returns 1.
func (c *Counter) Next() int {
	c.n++
	return c.n
}
