package netlaunch

import (
	"errors"
	"fmt"
	"io"
	"tridenttools/pkg/harpoon/harpoonpbv1"

	"github.com/fatih/color"
	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
	"google.golang.org/grpc"
)

func handleServicingResponseStream(stream grpc.ServerStreamingClient[harpoonpbv1.ServicingResponse]) error {
	for {
		resp, err := stream.Recv()
		if errors.Is(err, io.EOF) {
			log.Info("Install stream ended")
			break
		} else if err != nil {
			return fmt.Errorf("failed to receive installation response via Harpoon: %w", err)
		}

		err = handleServicingResponse(resp)
		if err != nil {
			return fmt.Errorf("failed to handle installation response via Harpoon: %w", err)
		}
	}

	return nil
}

var grpcHeader = color.New(color.FgGreen).SprintfFunc()("|GRPC|")

func handleServicingResponse(resp *harpoonpbv1.ServicingResponse) (err error) {
	switch payload := resp.Response.(type) {
	case *harpoonpbv1.ServicingResponse_Start:
		log.Infof("%s%s", grpcHeader, color.GreenString("[START]"))
	case *harpoonpbv1.ServicingResponse_Log:
		logEntry := payload.Log

		outLevel := logrus.InfoLevel
		switch logEntry.Level {
		case harpoonpbv1.LogLevel_LOG_LEVEL_TRACE:
			outLevel = logrus.TraceLevel
		case harpoonpbv1.LogLevel_LOG_LEVEL_DEBUG:
			outLevel = logrus.DebugLevel
		case harpoonpbv1.LogLevel_LOG_LEVEL_INFO:
			outLevel = logrus.InfoLevel
		case harpoonpbv1.LogLevel_LOG_LEVEL_WARN:
			outLevel = logrus.WarnLevel
		case harpoonpbv1.LogLevel_LOG_LEVEL_ERROR:
			outLevel = logrus.ErrorLevel
		}

		if outLevel <= log.DebugLevel {
			target := color.YellowString("[%s]", logEntry.Target)
			text := fmt.Sprintf("%s%s %s", grpcHeader, target, logEntry.Message)
			record := log.WithField("module", logEntry.Module)

			if logEntry.Location != nil {
				record = record.WithFields(log.Fields{
					"file": logEntry.Location.Path,
					"line": logEntry.Location.Line,
				})
			}

			record.Log(outLevel, text)
		}
	case *harpoonpbv1.ServicingResponse_FinalStatus:
		var errStr string
		level := logrus.InfoLevel
		if tridentError := payload.FinalStatus.GetError(); tridentError != nil {
			level = logrus.ErrorLevel
			errStr = fmt.Sprintf("\n%s", tridentError.FullBody)
			err = fmt.Errorf("operation failed with error kind %s:%s: %s", tridentError.Kind, tridentError.Subkind, tridentError.FullBody)
		}

		log.StandardLogger().Log(level, fmt.Sprintf(
			"%s%s %s%s",
			grpcHeader,
			color.MagentaString("[STATUS]"),
			payload.FinalStatus.Status.String(),
			errStr,
		))

		if payload.FinalStatus.GetRebootEnqueued() {
			log.Info("Trident will reboot the system to complete the operation")
		}
	default:
		log.Warnf("Received unknown response type from Harpoon: %T", payload)
	}

	return
}
