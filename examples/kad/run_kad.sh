#!/bin/bash

set -e

n_members="$1"

for i in $(seq 0 $(expr $n_members - 1)); do
   cargo run  --example kad $i $n_members 1> node$i.log &
done
