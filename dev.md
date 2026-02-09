## how to build image (local)

```bash
docker build -t {image_name} .
```

## build and push multi-platform (linux)

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t rich239/{image_name}:0.0.1 -t rich239/{image_name}:latest --push .
```

## push existing local image

```bash
docker tag merge-playground rich239/{image_name}:0.0.1
docker tag merge-playground rich239/{image_name}:latest
```

## push to docker hub (example)

```bash
docker push rich239/{image_name}:0.0.1
docker push rich239/{image_name}:latest
```

## pull image from repo

```bash
docker pull rich239/{image_name}:latest
```

_or you can run it using_

```bash
docker run -p 50051:50051 {image_name}
```
