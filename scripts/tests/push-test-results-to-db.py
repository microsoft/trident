import time
import requests
import os
import argparse
import xml.etree.ElementTree as ET
from azure.identity import DefaultAzureCredential


def get_token():
    app_registration_id = "13a078bd-0bae-4c0a-b691-df710bc234c1"
    credential = DefaultAzureCredential()
    token = None

    for i in range(5):
        try:
            token = credential.get_token(app_registration_id)
            break
        except Exception as e:
            print(f"Token retrieval failed: {e}")
            print("Retrying...")
            time.sleep(2**i)

    if token is None:
        raise Exception("Failed to retrieve token after multiple attempts")

    return token.token


def extract_test_type_name(junit_file_path, test_type_prefix=""):
    """Extract test suite name from JUnit XML file and prepend with test type prefix."""
    try:
        tree = ET.parse(junit_file_path)
        root = tree.getroot()
        
        # Look for testsuite element (could be direct child or under testsuites)
        testsuite = root.find('.//testsuite')
        if testsuite is not None and 'name' in testsuite.attrib:
            base_name = testsuite.attrib['name']
        else:
            # Fallback: use filename without extension
            filename = os.path.basename(junit_file_path)
            base_name = os.path.splitext(filename)[0]
    except Exception as e:
        print(f"Warning: Could not parse test suite name from {junit_file_path}: {e}")
        # Fallback: use filename without extension
        filename = os.path.basename(junit_file_path)
        base_name = os.path.splitext(filename)[0]
    
    # Prepend test type prefix if provided
    if test_type_prefix:
        return f"{test_type_prefix}-{base_name}"
    else:
        return base_name


def push_test_result_junit(test_suite, test_type, arch, build_number, run_pipeline_id, junit_file_path):
    url = "https://azlinux-api-management.azure-api.net/di/staging/image_release/push_test_result_junit"
    headers = {
        "Authorization": f"Bearer {get_token()}",
    }

    data = {
        "test_suite": test_suite,
        "arch": arch,
        "build_number": build_number,
        "run_pipeline_id": run_pipeline_id,
    }
    with open(junit_file_path, "rb") as junit_file:
        files = {
            "junit_xml": junit_file,
        }
        OVERRIDE_TEST_TYPE = True
        if OVERRIDE_TEST_TYPE:
            data["test_type_override"] = test_type
        response = requests.post(url, headers=headers, data=data, files=files)

        print(response.text)
        response.raise_for_status()
        return response.json()

def parse_args():
    parser = argparse.ArgumentParser(description="Push test results to database using API")
    parser.add_argument("--arch", required=True, help="Architecture")
    parser.add_argument("--build_number", required=True, help="Build number")
    parser.add_argument("--run_pipeline-id", required=True, help="Run pipeline ID")
    parser.add_argument("--junits_dir", required=True, help="Directory containing JUnit XML files")
    parser.add_argument("--is_staging", required=True, help="Whether this is staging environment")
    parser.add_argument("--test_suite", required=True, help="Test suite name")
    parser.add_argument("--test_type_prefix", required=False, default="", help="Prefix to prepend to test type names")
    return parser.parse_args()


def main():
    args = parse_args()
    arch = args.arch
    build_number = args.build_number
    run_pipeline_id = args.run_pipeline_id
    junits_dir = args.junits_dir
    test_suite = args.test_suite
    test_type_prefix = args.test_type_prefix

    print("Architecture: ", arch)
    print("Build number: ", build_number)
    print("Run pipeline ID: ", run_pipeline_id)
    print("Junits directory: ", junits_dir)
    print("Test suite: ", test_suite)
    print("Test type prefix: ", test_type_prefix)

    if args.is_staging.lower() == 'false':
        is_staging = False
    elif args.is_staging.lower() == 'true':
        is_staging = True
    print("Is staging: ", is_staging)

    for junit_file in os.listdir(junits_dir):
        if junit_file.endswith('.xml'):
            print("Processing: ", junit_file)
            junit_path = os.path.join(junits_dir, junit_file)
            
            # Extract test suite name from XML file and prepend prefix
            test_type_name = extract_test_type_name(junit_path, test_type_prefix)
            print(f"Extracted test type name: {test_type_name}")
            
            try:
                result = push_test_result_junit(test_suite, test_type_name, arch, build_number, run_pipeline_id, junit_path)
                print(f"Successfully pushed {junit_file}: {result}")
            except Exception as e:
                print(f"Failed to push {junit_file}: {e}")


if __name__ == "__main__":
    main()
