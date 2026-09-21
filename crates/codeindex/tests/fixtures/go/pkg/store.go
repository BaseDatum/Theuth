package pkg

import "fmt"

// Store holds items.
type Store struct {
	items []string
}

// Getter reads.
type Getter interface {
	Get(k string) string
}

// NewStore makes one.
func NewStore() *Store {
	return &Store{}
}

// Get returns k.
func (s *Store) Get(k string) string {
	return fmt.Sprint(k)
}

func helper(x int) int {
	return x + 1
}

func run() int {
	s := NewStore()
	return helper(len(s.Get("a")))
}
