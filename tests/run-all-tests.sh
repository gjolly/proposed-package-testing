#!/bin/bash -eu

for test in ./tests/*-test.sh; do
    if [[ -x "$test" ]]; then
        echo "Running test: $test"
        "$test"
        if [[ $? -ne 0 ]]; then
            echo "Test $test failed"
            exit 1
        fi
    else
        echo "Skipping non-executable test: $test"
    fi
done

echo "All tests passed successfully."