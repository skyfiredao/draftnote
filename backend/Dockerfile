FROM alpine:3.20
RUN apk add --no-cache git
COPY draftnote-backend /usr/local/bin/draftnote-backend
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["draftnote-backend"]
