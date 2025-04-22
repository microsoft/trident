package devops

import "fmt"

// Groups function as a stack, so we keep track of the groups in a stack.
var groups = make([]*Group, 0)

// Opens a new group and adds it to the stack.
func OpenGroup(name string) *Group {
	newGroup := &Group{}
	groups = append(groups, newGroup)
	logCreateGroup(name)
	return newGroup
}

func logCreateGroup(name string) {
	fmt.Printf("##[group]%s\n", name)
}

func logEndGroup() {
	fmt.Println("##[endgroup]")
}

type Group struct{}

// Closes the group and removes all groups above it from the stack.
// This is done by popping the stack until we reach the group we want to close.
func (g *Group) Close() {
	var index int = len(groups) - 1
	for index >= 0 {
		// Pop the last group from the stack
		last := groups[index]
		groups = groups[:index]
		logEndGroup()
		if last == g {
			break
		}
		index--
	}
}
