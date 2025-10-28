#!/usr/bin/env python3
"""
Flash OS Workflow Script

This script orchestrates the complete workflow for flashing an OS image:
1. Parse host configuration to extract image URL
2. Download the OS image from the URL
3. Upload the image to bmi-prep-agent-server via bmi-prep-agent-cli
4. Trigger the flash OS operation via gRPC

Usage:
    python flash_os_workflow.py --host-config <path>
"""

"""
Prequisites:
- bmi-prep-agent-cli in trident/artifacts/test-image/
- bmi-prep-agent-server in trident/artifacts/test-image/
- this script in trident/artifacts/test-image/
- regular.cosi in trident/artifacts/test-image/, rename from the modified
trident-testimage.cosi from test-images jiria/hpc branch
- harpoon2-server in trident/artifacts/test-image/
"""

import argparse
import json
import logging
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional, Tuple
from urllib.parse import urlparse

try:
    import requests
    import yaml
except ImportError:
    print("ERROR: Required dependencies not installed.")
    print("Please install: pip install requests pyyaml")
    sys.exit(1)

# Configure logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)


class FlashOsWorkflow:
    """Orchestrates the OS flashing workflow"""

    def __init__(
        self,
        host_config_path: str,
        download_dir: Optional[str] = None,
    ):
        """
        Initialize the workflow

        Args:
            host_config_path: Path to the host configuration YAML file
            download_dir: Directory to download images to (default: /tmp)
        """
        self.host_config_path = Path(host_config_path)
        self.download_dir = Path(download_dir or "/tmp")
        self.host_config = None
        self.image_url = None
        self.downloaded_file = None

    def parse_host_config(self) -> str:
        """
        Parse the host configuration and extract the image URL

        Returns:
            The image URL extracted from the configuration

        Raises:
            FileNotFoundError: If the host config file doesn't exist
            ValueError: If the image URL cannot be extracted
        """
        logger.info(f"Parsing host configuration from {self.host_config_path}")

        if not self.host_config_path.exists():
            raise FileNotFoundError(
                f"Host config file not found: {self.host_config_path}"
            )

        with open(self.host_config_path, "r") as f:
            self.host_config = yaml.safe_load(f)

        # Extract image URL from config
        # Expected structure: image.url
        try:
            self.image_url = self.host_config["image"]["url"]
            logger.info(f"Extracted image URL: {self.image_url}")
        except KeyError as e:
            raise ValueError(f"Failed to extract image URL from config: {e}")

        return self.image_url

    def download_image(self, url: str) -> Path:
        """
        Download the OS image from the URL

        Args:
            url: URL to download the image from

        Returns:
            Path to the downloaded file

        Raises:
            RuntimeError: If download fails
        """
        logger.info(f"Downloading image from {url}")

        # Parse URL to get filename
        parsed_url = urlparse(url)
        filename = os.path.basename(parsed_url.path)
        if not filename:
            filename = "downloaded_image.img"

        download_path = self.download_dir / filename

        # Check if file already exists
        if download_path.exists():
            logger.info(f"File already exists at {download_path}, skipping download")
            self.downloaded_file = download_path
            return download_path

        # Download the file with progress
        try:
            response = requests.get(url, stream=True, timeout=30)
            response.raise_for_status()

            total_size = int(response.headers.get("content-length", 0))
            downloaded = 0

            with open(download_path, "wb") as f:
                for chunk in response.iter_content(chunk_size=4*1024*1024):
                    if chunk:
                        f.write(chunk)
                        downloaded += len(chunk)
                        if total_size > 0:
                            percent = (downloaded / total_size) * 100
                            logger.info(
                                f"Download progress: {percent:.1f}% ({downloaded}/{total_size} bytes)"
                            )

            logger.info(f"Download completed: {download_path} ({downloaded} bytes)")
            self.downloaded_file = download_path
            return download_path

        except requests.exceptions.RequestException as e:
            raise RuntimeError(f"Failed to download image: {e}")

    def upload_image(self, file_path: Path) -> bool:
        """
        Upload the image to bmi-prep-agent-server using bmi-prep-agent-cli

        Args:
            file_path: Path to the file to upload

        Returns:
            True if upload succeeded, False otherwise

        Raises:
            RuntimeError: If upload fails
        """
        logger.info(f"Uploading image {file_path} to BPA server")

        # Find bmi-prep-agent-cli executable
        cli_path = "/usr/bin/bmi-prep-agent-cli"
        logger.info(f"Using CLI: {cli_path}")

        # Build the upload command
        cmd = [
            str(cli_path),
            "upload",
            "--file",
            str(file_path),
        ]

        logger.info(f"Running command: {' '.join(cmd)}")

        try:
            # Run the upload command
            process = subprocess.Popen(
                cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
            )

            # Stream output in real-time
            for line in process.stdout:
                logger.info(f"[CLI] {line.rstrip()}")

            # Wait for completion
            return_code = process.wait()

            if return_code != 0:
                stderr = process.stderr.read()
                logger.error(f"Upload failed with return code {return_code}")
                logger.error(f"Error output: {stderr}")
                raise RuntimeError(f"Upload failed: {stderr}")

            logger.info("Upload completed successfully")
            return True

        except subprocess.SubprocessError as e:
            raise RuntimeError(f"Failed to run upload command: {e}")

    def flash_os(self) -> bool:
        """
        Trigger the flash OS operation via gRPC

        Returns:
            True if flash OS succeeded, False otherwise

        Raises:
            RuntimeError: If flash OS operation fails
        """
        logger.info("Triggering flash OS operation")

        cli_path = "/usr/bin/bmi-prep-agent-cli"
        
        # Build the flash OS command
        cmd = [str(cli_path), "flash-os", "--file-id", "foobar"]

        logger.info(f"Running command: {' '.join(cmd)}")

        try:
            # Run the flash OS command
            process = subprocess.Popen(
                cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
            )

            # Stream output in real-time
            for line in process.stdout:
                logger.info(f"[CLI] {line.rstrip()}")

            # Wait for completion
            return_code = process.wait()

            if return_code != 0:
                stderr = process.stderr.read()
                logger.error(f"Flash OS failed with return code {return_code}")
                logger.error(f"Error output: {stderr}")
                raise RuntimeError(f"Flash OS failed: {stderr}")

            logger.info("Flash OS completed successfully")
            return True

        except subprocess.SubprocessError as e:
            raise RuntimeError(f"Failed to run flash OS command: {e}")

    def run(self) -> bool:
        """
        Execute the complete workflow

        Returns:
            True if all steps succeeded, False otherwise
        """
        try:
            logger.info("=" * 60)
            logger.info("Starting Flash OS Workflow")
            logger.info("=" * 60)

            # Step 1: Parse host configuration
            logger.info("Step 1/4: Parsing host configuration")
            self.parse_host_config()

            # Step 2: Download image
            logger.info("Step 2/4: Downloading OS image")
            downloaded_path = self.download_image(self.image_url)

            # Step 3: Upload image
            logger.info("Step 3/4: Uploading image to server")
            self.upload_image(downloaded_path)

            # Step 4: Flash OS
            logger.info("Step 4/4: Flashing OS to device")
            self.flash_os()

            logger.info("=" * 60)
            logger.info("Flash OS Workflow Completed Successfully!")
            logger.info("=" * 60)
            return True

        except Exception as e:
            logger.error("=" * 60)
            logger.error(f"Flash OS Workflow Failed: {e}")
            logger.error("=" * 60)
            return False


def main():
    """Main entry point"""
    parser = argparse.ArgumentParser(
        description="Flash OS Workflow - Orchestrates OS image download, upload, and flashing",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Basic usage with host config
  python flash_os_workflow.py --host-config /path/to/config.yaml
        """,
    )

    parser.add_argument(
        "--host-config", required=True, help="Path to the host configuration YAML file"
    )

    parser.add_argument(
        "--download-dir", help="Directory to download images to (default: /tmp)"
    )

    parser.add_argument(
        "--verbose", "-v", action="store_true", help="Enable verbose logging"
    )

    args = parser.parse_args()

    # Set logging level
    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    # Create and run workflow
    workflow = FlashOsWorkflow(
        host_config_path=args.host_config,
        download_dir=args.download_dir,
    )

    success = workflow.run()
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
