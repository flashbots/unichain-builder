# Flashblocks Builder

> todo: docs

Implemented using [rblib](https://github.com/flashbots/rblib).

## Manual Testing in Builder Playground

Here we will start builder playground, configure it to use Flashblocks as an external builder, configure prometheus and grafana, start Flashblocks builder and spam the node with transactions using contender. This will give us an end-to-end manual testing environment.

### Setup

#### 1. Compile flashblocks in release mode

```shell
cargo build --release
```

#### 2. Clone builder-playground

```shell
git clone https://github.com/flashbots/builder-playground.git
```

#### 3. Install contender

```shell
cargo install --git https://github.com/flashbots/contender --locked
```

#### 4. Configure Grafana and Prometheus

Create the following `yaml` files that will spin up `grafana` and `prometheus` servers:

$\quad$ **4.a `docker-compose.yml`**

   ```yaml
   services:
     prometheus:
       image: prom/prometheus
       ports:
         - "19090:9090"
       volumes:
         - ./prometheus.yml:/etc/prometheus/prometheus.yml
       command:
         - '--config.file=/etc/prometheus/prometheus.yml'

     grafana:
       image: grafana/grafana
       ports:
         - "3000:3000"
       environment:
         - GF_SECURITY_ADMIN_USER=admin
         - GF_SECURITY_ADMIN_PASSWORD=admin
       depends_on:
         - prometheus
   ```

$\quad$ **4.b `prometheus.yml`**

   ```yaml
   global:
     scrape_interval: 5s

   scrape_configs:
     - job_name: 'my_exporter'
       static_configs:
         - targets: ['host.docker.internal:5559']
   ```

### Running

Remember to delete temporary directories created by `builder-playground` and `flashblocks` between runs, otherwise you will get errors about mismatched genesis block hashes.
For example on macOS use those commands:

```shell
rm -rf "~/Library/Caches/reth"
rm -rf "~/Library/Application Support/reth"
rm -rf "~/.playground"
```

On other platforms you can figure out the equivalent paths from the logs emitted by `builder-playground` and `flashblocks`.

#### 1. Run builder playground

Inside the cloned `builder-playground` repo from setup step 3 run:

```shell
./builder-playground cook opstack --with-prometheus --external-builder http://host.docker.internal:7891
```

#### 2. Run Flashblocks builder

Run the `release` version of `flashblocks` binary from this repo with `--builder.playground` flag.
> _`debug` builds will work too but builder's performance will be **severely** degraded._

```shell
RUST_LOG=debug ./target/release/flashblocks node --builder.playground --metrics=localhost:5559
```

#### 3. Start Prometherus and Grafana

Go to the directory where files from setup step 4 were created and run

```shell
docker compose up
```

Open `http://localhost:3000` in your browser to navigate to Grafana, then:

1. Add a new Prometheus data source that points to `http://prometheus:9090`
2. Create a new dashboard by importing the `grafana/standard.json` file from this repo.

#### 4. Generate spam transactions

Non-reverting transactions:

```shell
target/debug/contender spam --tps=150 -l -r http://localhost:2222 --min-balance 1000.5ether
```

Reverting transactions:

```shell
target/debug/contender spam --tps 30 -l -r http://localhost:2222 --min-balance 1000.5ether revert
```
