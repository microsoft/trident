import time
import requests
import os
import argparse
import xml.etree.ElementTree as ET
from azure.identity import DefaultAzureCredential

APP_REG_ID = "13a078bd-0bae-4c0a-b691-df710bc234c1"
TOKEN_SCOPE = f"api://{APP_REG_ID}/.default"

def get_token():
    credential = DefaultAzureCredential()
    token = None
    for i in range(5):
        try:
            token = credential.get_token(TOKEN_SCOPE)
            break
        except Exception as e:
            print(f"Token retrieval failed: {e}")
            print("Retrying...")
            time.sleep(2**i)
    if token is None:
        raise Exception("Failed to retrieve token after multiple attempts")
    return token.token

def extract_test_type_name(junit_file_path, test_type_prefix=""):
    try:
        tree = ET.parse(junit_file_path)
        root = tree.getroot()
        testsuite = root.find('.//testsuite')
        if testsuite is not None and 'name' in testsuite.attrib:
            base_name = testsuite.attrib['name']
        else:
            base_name = os.path.splitext(os.path.basename(junit_file_path))[0]
    except Exception as e:
        print(f"Warning: Could not parse test suite name from {junit_file_path}: {e}")
        base_name = os.path.splitext(os.path.basename(junit_file_path))[0]
    return f"{test_type_prefix}-{base_name}" if test_type_prefix else base_name

def push_test_result_junit(test_suite, test_type, arch, build_number, run_pipeline_id, junit_file_path):
    url = "https://azlinux-api-management.azure-api.net/di/staging/image_release/push_test_result_junit"
    headers = {"Authorization": f"Bearer {get_token()}"}
    data = {
        "test_suite": test_suite,
        "arch": arch,
        "build_number": build_number,
        "run_pipeline_id": run_pipeline_id,
        "test_type_override": test_type,
    }
    with open(junit_file_path, "rb") as junit_file:
        files = {"junit_xml": junit_file}
        resp = requests.post(url, headers=headers, data=data, files=files)
        print(resp.text)
        resp.raise_for_status()
        return resp.json()

def parse_args():
    p = argparse.ArgumentParser(description="Push test results to database using API")
    p.add_argument("--arch", required=True)
    p.add_argument("--build_number", required=True)
    p.add_argument("--run_pipeline-id", required=True)
    p.add_argument("--junits_dir", required=True)
    p.add_argument("--is_staging", required=True)
    p.add_argument("--test_suite", required=True)
    p.add_argument("--test_type_prefix", default="")
    return p.parse_args()

def main():
    args = parse_args()
    print("Architecture:", args.arch)
    print("Build number:", args.build_number)
    print("Run pipeline ID:", args.run_pipeline_id)
    print("Junits directory:", args.junits_dir)
    print("Test suite:", args.test_suite)
    print("Test type prefix:", args.test_type_prefix)
    is_staging = args.is_staging.lower() == 'true'
    print("Is staging:", is_staging)

    for name in os.listdir(args.junits_dir):
        if name.endswith(".xml"):
            path = os.path.join(args.junits_dir, name)
            print("Processing:", path)
            test_type_name = extract_test_type_name(path, args.test_type_prefix)
            try:
                result = push_test_result_junit(
                    args.test_suite, test_type_name, args.arch,
                    args.build_number, args.run_pipeline_id, path
                )
                print(f"Successfully pushed {name}: {result}")
            except Exception as e:
                print(f"Failed to push {name}: {e}")

if __name__ == "__main__":
    main()
