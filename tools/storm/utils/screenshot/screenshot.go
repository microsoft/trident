// Package screenshot captures VM console screenshots through libvirt.
//
// It replaces the `sudo virsh screenshot` + ImageMagick `convert` pipeline the
// legacy capture-screenshot script used: the image comes over the existing
// libvirt connection and the PPM that QEMU produces is converted in-process, so
// a capture needs no elevated privileges and no tools installed on the host.
package screenshot

import (
	"bufio"
	"bytes"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"io"

	"github.com/digitalocean/go-libvirt"
)

var (
	// pngMagic and ppmMagic identify the two formats a libvirt console
	// screenshot can arrive in.
	pngMagic = []byte{0x89, 'P', 'N', 'G'}
	ppmMagic = []byte("P6")
)

// maxPpmDimension bounds each PPM header dimension. A console framebuffer is
// far smaller than this, so the only headers it rejects are corrupt ones -- and
// rejecting them matters, because screenshots are captured while diagnosing a
// failure, where panicking or exhausting memory would destroy the diagnostic
// the capture exists to provide.
const maxPpmDimension = 1 << 16

// CapturePng writes a PNG screenshot of the domain's first console to out.
//
// The format is detected from the image's magic bytes rather than from the mime
// type libvirt reports, for two reasons. go-libvirt's generated
// DomainScreenshot cannot return the mime at all: with an incoming stream,
// requestStream hands back the end-of-stream response, whose payload is empty,
// so decoding the mime always fails with EOF even though the image itself
// arrived intact. And the format genuinely varies - a QXL/SPICE guest returns
// PNG while a plain VGA guest returns PPM - so it has to be sniffed regardless.
func CapturePng(lv *libvirt.Libvirt, dom libvirt.Domain, out io.Writer) error {
	var raw bytes.Buffer
	// The mime return value is discarded: see above.
	_, err := lv.DomainScreenshot(dom, &raw, 0, 0)
	if err != nil && raw.Len() == 0 {
		return fmt.Errorf("failed to capture screenshot over libvirt: %w", err)
	}
	if raw.Len() == 0 {
		return fmt.Errorf("libvirt returned an empty screenshot for domain %q", dom.Name)
	}

	switch {
	case bytes.HasPrefix(raw.Bytes(), pngMagic):
		if _, err := io.Copy(out, &raw); err != nil {
			return fmt.Errorf("failed to write PNG screenshot: %w", err)
		}
		return nil
	case bytes.HasPrefix(raw.Bytes(), ppmMagic):
		return PpmToPng(&raw, out)
	default:
		return fmt.Errorf("unrecognized screenshot format, first bytes: %x", raw.Bytes()[:min(8, raw.Len())])
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// PpmToPng converts a binary (P6) PPM image to PNG.
func PpmToPng(r io.Reader, w io.Writer) error {
	br := bufio.NewReader(r)

	magic, err := readToken(br)
	if err != nil {
		return fmt.Errorf("failed to read PPM magic: %w", err)
	}
	if magic != "P6" {
		return fmt.Errorf("unsupported PPM format %q, expected binary P6", magic)
	}

	width, err := readIntToken(br)
	if err != nil {
		return fmt.Errorf("failed to read PPM width: %w", err)
	}
	height, err := readIntToken(br)
	if err != nil {
		return fmt.Errorf("failed to read PPM height: %w", err)
	}
	maxValue, err := readIntToken(br)
	if err != nil {
		return fmt.Errorf("failed to read PPM max value: %w", err)
	}

	if maxValue < 1 || maxValue > 255 {
		return fmt.Errorf("unsupported PPM max value %d, expected 1-255", maxValue)
	}
	if width < 1 || height < 1 {
		return fmt.Errorf("invalid PPM dimensions %dx%d", width, height)
	}
	if width > maxPpmDimension || height > maxPpmDimension {
		return fmt.Errorf("PPM dimensions %dx%d exceed the maximum of %d", width, height, maxPpmDimension)
	}

	// Exactly one whitespace byte separates the header from the pixel data.
	if _, err := br.ReadByte(); err != nil {
		return fmt.Errorf("failed to read PPM header terminator: %w", err)
	}

	pixels := make([]byte, width*height*3)
	if _, err := io.ReadFull(br, pixels); err != nil {
		return fmt.Errorf("failed to read PPM pixel data (%dx%d): %w", width, height, err)
	}

	img := image.NewRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			i := (y*width + x) * 3
			img.SetRGBA(x, y, color.RGBA{R: pixels[i], G: pixels[i+1], B: pixels[i+2], A: 0xFF})
		}
	}

	if err := png.Encode(w, img); err != nil {
		return fmt.Errorf("failed to encode PNG: %w", err)
	}
	return nil
}

// readToken reads the next whitespace-delimited token, skipping comments, which
// a PPM header may carry between any two tokens.
func readToken(r *bufio.Reader) (string, error) {
	var token []byte
	for {
		b, err := r.ReadByte()
		if err != nil {
			return "", err
		}

		switch {
		case b == '#':
			// Comments run to end of line.
			for b != '\n' {
				if b, err = r.ReadByte(); err != nil {
					return "", err
				}
			}
		case isPpmSpace(b):
			if len(token) > 0 {
				// The delimiter after the final header token belongs to the
				// caller, which consumes exactly one byte before the pixels.
				if err := r.UnreadByte(); err != nil {
					return "", err
				}
				return string(token), nil
			}
		default:
			token = append(token, b)
		}
	}
}

func readIntToken(r *bufio.Reader) (int, error) {
	token, err := readToken(r)
	if err != nil {
		return 0, err
	}

	value := 0
	for _, c := range token {
		if c < '0' || c > '9' {
			return 0, fmt.Errorf("expected a number, got %q", token)
		}
		// Reject before the multiply rather than letting int wrap silently: a
		// wrapped dimension passes the positivity check and then overflows the
		// pixel-buffer size calculation.
		if value > (maxPpmDimension-int(c-'0'))/10 {
			return 0, fmt.Errorf("number %q exceeds the maximum of %d", token, maxPpmDimension)
		}
		value = value*10 + int(c-'0')
	}
	return value, nil
}

func isPpmSpace(b byte) bool {
	return b == ' ' || b == '\t' || b == '\n' || b == '\r' || b == '\v' || b == '\f'
}
