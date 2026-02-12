## how to build image (local)

```bash
docker build -t {image_name} .
```

## build and push multi-platform (linux)

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t {dockerhub_id}/{image_name}:0.0.1 -t {dockerhub_id}/{image_name}:latest --push .
```

**Example:**

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t rich239/swift-tree:0.0.1 -t rich239/swift-tree:latest --push .
```

## push existing local image

```bash
docker tag {image_name} {dockerhub_id}/{image_name}:0.0.1
docker tag {image_name} {dockerhub_id}/{image_name}:latest
```

**Example:**

```bash
docker tag swift-tree rich239/swift-tree:0.0.1
docker tag swift-tree rich239/swift-tree:latest
```

## push to docker hub

```bash
docker push {dockerhub_id}/{image_name}:0.0.1
docker push {dockerhub_id}/{image_name}:latest
```

**Example:**

```bash
docker push rich239/swift-tree:0.0.1
docker push rich239/swift-tree:latest
```

## pull image from repo

```bash
docker pull {dockerhub_id}/{image_name}:latest
```

**Example:**

```bash
docker pull rich239/swift-tree:latest
```

_or you can run it using_

```bash
docker run -p 50051:50051 {image_name}
```

## debugging

**since each log entry is on a seperate line using**

```bash
docker logs {container_id} -n
```

**is very helpful as you control how many logs you view**
