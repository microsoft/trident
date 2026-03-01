package netlaunch

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"tridenttools/pkg/tridentgrpc"
	"tridenttools/pkg/tridentgrpc/tridentpbv1"
	"tridenttools/pkg/tridentgrpc/tridentpbv1preview"

	"github.com/fatih/color"
	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
	"google.golang.org/grpc"
)

func doGrpcInstall(ctx context.Context, conn net.Conn, hostConfiguration string) error {
	tridentClient, err := tridentgrpc.NewTridentClientFromNetworkConnection(conn)
	if err != nil {
		return fmt.Errorf("failed to create Trident gRPC client from RCP connection: %w", err)
	}
	defer tridentClient.Close()

	stream, err := tridentClient.Install(ctx, &tridentpbv1preview.InstallRequest{
		Stage: &tridentpbv1preview.StageInstallRequest{
			Config: &tridentpbv1preview.HostConfiguration{
				Config: hostConfiguration,
			},
		},
		Finalize: &tridentpbv1preview.FinalizeInstallRequest{
			Reboot: &tridentpbv1.RebootManagement{
				Handling: tridentpbv1.RebootHandling_TRIDENT_HANDLES_REBOOT,
			},
		},
	})
	if err != nil {
		return fmt.Errorf("failed to start installation via gRPC: %w", err)
	}

	err = handleServicingResponseStream(stream)
	if err != nil {
		return fmt.Errorf("error during installation via gRPC: %w", err)
	}

	return nil
}

func doGrpcStream(ctx context.Context, conn net.Conn, imageUrl string, imageHash string) error {
	tridentClient, err := tridentgrpc.NewTridentClientFromNetworkConnection(conn)
	if err != nil {
		return fmt.Errorf("failed to create Trident gRPC client from RCP connection: %w", err)
	}
	defer tridentClient.Close()

	stream, err := tridentClient.StreamingServiceClient.StreamDisk(ctx, &tridentpbv1.StreamDiskRequest{
		ImageUrl:  imageUrl,
		ImageHash: &imageHash,
		Reboot: &tridentpbv1.RebootManagement{
			Handling: tridentpbv1.RebootHandling_TRIDENT_HANDLES_REBOOT,
		},
	})
	if err != nil {
		return fmt.Errorf("failed to start streaming via gRPC: %w", err)
	}

	err = handleServicingResponseStream(stream)
	if err != nil {
		return fmt.Errorf("error during streaming via gRPC: %w", err)
	}

	return nil
}

func handleServicingResponseStream(stream grpc.ServerStreamingClient[tridentpbv1.ServicingResponse]) error {
	for {
		resp, err := stream.Recv()
		if errors.Is(err, io.EOF) {
			log.Info("Servicing stream ended")
			break
		} else if err != nil {
			return fmt.Errorf("failed to receive servicing response via gRPC: %w", err)
		}

		err = handleServicingResponse(resp)
		if err != nil {
			return fmt.Errorf("failed to handle servicing response via gRPC: %w", err)
		}
	}

	return nil
}

var grpcHeader = color.New(color.FgGreen).SprintfFunc()("|GRPC|")

func handleServicingResponse(resp *tridentpbv1.ServicingResponse) (err error) {
	switch payload := resp.Response.(type) {
	case *tridentpbv1.ServicingResponse_Started:
		log.Infof("%s%s", grpcHeader, color.GreenString("[START]"))
	case *tridentpbv1.ServicingResponse_Log:
		logEntry := payload.Log

		outLevel := logrus.InfoLevel
		switch logEntry.Level {
		case tridentpbv1.LogLevel_LOG_LEVEL_TRACE:
			outLevel = logrus.TraceLevel
		case tridentpbv1.LogLevel_LOG_LEVEL_DEBUG:
			outLevel = logrus.DebugLevel
		case tridentpbv1.LogLevel_LOG_LEVEL_INFO:
			outLevel = logrus.InfoLevel
		case tridentpbv1.LogLevel_LOG_LEVEL_WARN:
			outLevel = logrus.WarnLevel
		case tridentpbv1.LogLevel_LOG_LEVEL_ERROR:
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
	case *tridentpbv1.ServicingResponse_Completed:
		var statusStr string
		switch payload.Completed.Status {
		case tridentpbv1.StatusCode_STATUS_CODE_SUCCESS:
			statusStr = color.CyanString("SUCCESS")
		case tridentpbv1.StatusCode_STATUS_CODE_FAILURE:
			statusStr = color.RedString("FAILURE")
		case tridentpbv1.StatusCode_STATUS_CODE_UNSPECIFIED:
			statusStr = color.YellowString("UNSPECIFIED")
		}
		var errStr string
		level := logrus.InfoLevel
		if tridentError := payload.Completed.GetError(); tridentError != nil {
			level = logrus.ErrorLevel
			errStr = fmt.Sprintf("\n%s", tridentError.Message)
			err = fmt.Errorf("operation failed with error kind %s:%s: %s", tridentError.Kind, tridentError.Subkind, tridentError.Message)
		} else {
			switch payload.Completed.RebootStatus {
			case tridentpbv1.RebootStatus_REBOOT_NOT_REQUIRED:
				errStr = " - Reboot: not required"
			case tridentpbv1.RebootStatus_REBOOT_REQUIRED:
				errStr = " - Reboot: required"
			case tridentpbv1.RebootStatus_REBOOT_STARTED:
				errStr = " - Reboot: started by trident"
			}
		}

		log.StandardLogger().Log(level, fmt.Sprintf(
			"%s%s %s%s",
			grpcHeader,
			color.MagentaString("COMPLETED:"),
			statusStr,
			errStr,
		))
	default:
		log.Warnf("Received unknown response type from Trident: %T", payload)
	}

	return
}
