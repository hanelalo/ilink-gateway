.PHONY: build run test test-verbose vet clean

build:
	go build -o bin/feishu-gateway ./cmd/feishu-gateway

run:
	go run ./cmd/feishu-gateway

test:
	go test ./...

test-verbose:
	go test ./... -v

vet:
	go vet ./...

clean:
	rm -rf bin/
