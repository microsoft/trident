package reporter

import (
	"fmt"
	"strings"

	"golang.org/x/term"
)

const SEPARATOR_CHAR = "="

// Returns the width of the terminal. If it cannot be determined, it returns a
// default value of 160.
func termWidth() int {
	width, _, err := term.GetSize(0)
	if err != nil {
		return 160
	}
	return width
}

// Prints a separator line to the console with a title.
// The title is more or less left aligned.
//
// Example:
//
//	--- MyTitle ---------------------------------------
func printSeparatorWithTitle(title string) {
	width := termWidth()
	preTitle := strings.Repeat(SEPARATOR_CHAR, 3) + " "
	titleWidth := len(title) + len(preTitle)
	separatorWidth := width - titleWidth - 1
	if separatorWidth < 0 {
		separatorWidth = 0
	}
	// Print the title
	fmt.Printf("%s%s %s\n", preTitle, title, strings.Repeat(SEPARATOR_CHAR, separatorWidth))
}

// Prints a separator line to the console.
func printSeparator() {
	fmt.Printf("%s\n", strings.Repeat(SEPARATOR_CHAR, termWidth()))
}

// Prints a separator line to the console.
func printSeparatorChar(char string) {
	fmt.Printf("%s\n", strings.Repeat(char, termWidth()))
}

func simpleWordWrap(text string, maxWidth int) []string {
	lines := make([]string, 0)

	// Split the text into words
	words := strings.Split(text, " ")
	// Initialize the current line to the first word
	currentLine := words[0]
	for _, word := range words[1:] {
		// Check if adding the next word exceeds the max width
		if (len(currentLine) + len(word) + 1) > maxWidth {
			// Finalize the current line by adding it to the list
			lines = append(lines, currentLine)
			// Start a new current line with the current word
			currentLine = word
		} else {
			// Add the word to the current line
			currentLine += " "
			currentLine += word
		}
	}

	// Add the last line if it's not empty
	if currentLine != "" {
		lines = append(lines, currentLine)
	}

	return lines
}
