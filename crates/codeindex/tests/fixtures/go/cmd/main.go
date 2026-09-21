package main

import (
	"fmt"

	st "example.com/fixture/pkg"
	"example.com/fixture/nope"
)

func main() {
	var s *st.Store = st.NewStore()
	var g st.Getter = s
	fmt.Println(g.Get("a"), nope.X)
}
