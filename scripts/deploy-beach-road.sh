#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/deploy-beach-road.sh [options]

Build the beach-road binary and deploy it to an EC2 host. If no host or
instance id is supplied, a new Amazon Linux 2023 instance will be created,
prepared (Docker + Redis), and the service deployed automatically.

Key options:
  --region REGION        AWS region (required when using AWS operations)
  --aws-profile NAME     AWS CLI profile to use (optional)
  --host HOST            Existing SSH hostname / IP
  --instance-id ID       Existing EC2 instance id
  --key-name NAME        EC2 key pair name (required when creating instance)
  --ssh-key PATH         SSH private key file (required for SSH)
  --ssh-user NAME        SSH username (default: ec2-user)
  --remote-dir DIR       Remote install dir (default: /opt/beach-road)
  --instance-type TYPE   EC2 instance type (default: t3.small)
  --security-group ID    Reuse an existing security group rather than creating one
  --env-file PATH        Optional env file copied to beach-road.env
  --target TRIPLE        Cargo build target (default: x86_64-unknown-linux-gnu)
  --profile PROFILE      Cargo build profile (default: release)
  --allocate-eip         Ensure instance has a public IP by associating an Elastic IP
  --skip-build           Skip local cargo build step
  --dry-run              Print intended actions without executing
  -h, --help             Show this help text

Environment overrides:
  AWS_REGION, AWS_PROFILE, BEACH_ROAD_INSTANCE_ID, BEACH_ROAD_HOST,
  BEACH_ROAD_KEY_NAME, BEACH_ROAD_SSH_KEY_PATH, BEACH_ROAD_SSH_USER,
  BEACH_ROAD_REMOTE_DIR, BEACH_ROAD_SECURITY_GROUP_ID,
  BEACH_ROAD_INSTANCE_TYPE, CARGO_BUILD_TARGET, CARGO_BUILD_PROFILE.

Prerequisites:
  - Rust toolchain and requested target installed
  - AWS CLI (+ python3) for AWS operations
  - ssh/scp available locally
  - When creating a new instance: existing EC2 key pair matching --key-name
USAGE
}

PROJECT_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
APP_DIR="$PROJECT_ROOT/apps/beach-road"
TARGET=${CARGO_BUILD_TARGET:-x86_64-unknown-linux-gnu}
PROFILE=${CARGO_BUILD_PROFILE:-release}
REMOTE_DIR=${BEACH_ROAD_REMOTE_DIR:-/opt/beach-road}
REMOTE_USER=${BEACH_ROAD_SSH_USER:-ec2-user}
SSH_KEY=${BEACH_ROAD_SSH_KEY_PATH:-}
INSTANCE_ID=${BEACH_ROAD_INSTANCE_ID:-}
HOST=${BEACH_ROAD_HOST:-}
AWS_REGION=${AWS_REGION:-}
AWS_PROFILE=${AWS_PROFILE:-}
INSTANCE_TYPE=${BEACH_ROAD_INSTANCE_TYPE:-t3.small}
SECURITY_GROUP_ID=${BEACH_ROAD_SECURITY_GROUP_ID:-}
KEY_NAME=${BEACH_ROAD_KEY_NAME:-}
ENV_FILE=""
SKIP_BUILD=0
DRY_RUN=0
ENSURE_EIP=0

while [[ $# -gt 0 ]]; do
  case $1 in
    --env-file)
      ENV_FILE=$2; shift 2 ;;
    --host)
      HOST=$2; shift 2 ;;
    --instance-id)
      INSTANCE_ID=$2; shift 2 ;;
    --region)
      AWS_REGION=$2; shift 2 ;;
    --aws-profile)
      AWS_PROFILE=$2; shift 2 ;;
    --key-name)
      KEY_NAME=$2; shift 2 ;;
    --ssh-key)
      SSH_KEY=$2; shift 2 ;;
    --ssh-user)
      REMOTE_USER=$2; shift 2 ;;
    --remote-dir)
      REMOTE_DIR=$2; shift 2 ;;
    --instance-type)
      INSTANCE_TYPE=$2; shift 2 ;;
    --security-group)
      SECURITY_GROUP_ID=$2; shift 2 ;;
    --target)
      TARGET=$2; shift 2 ;;
    --profile)
      PROFILE=$2; shift 2 ;;
    --allocate-eip)
      ENSURE_EIP=1; shift ;;
    --skip-build)
      SKIP_BUILD=1; shift ;;
    --dry-run)
      DRY_RUN=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1 ;;
  esac
done

if [[ -n $ENV_FILE && ! -f $ENV_FILE ]]; then
  echo "Env file not found: $ENV_FILE" >&2
  exit 1
fi

if ! command -v cargo >/dev/null; then
  echo "cargo not found in PATH." >&2
  exit 1
fi

for tool in ssh scp; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool not found in PATH." >&2
    exit 1
  fi
done

aws_required=0
if [[ -n $INSTANCE_ID || (-z $HOST && -z $INSTANCE_ID) ]]; then
  aws_required=1
fi
if [[ $aws_required -eq 1 ]]; then
  if [[ -z $AWS_REGION ]]; then
    echo "--region (or AWS_REGION) is required for AWS operations." >&2
    exit 1
  fi
  if ! command -v aws >/dev/null; then
    echo "aws CLI not found in PATH." >&2
    exit 1
  fi
  if ! command -v python3 >/dev/null; then
    echo "python3 is required for JSON parsing." >&2
    exit 1
  fi
fi

AWS_ARGS=("--region" "$AWS_REGION")
if [[ -n $AWS_PROFILE ]]; then
  AWS_ARGS+=("--profile" "$AWS_PROFILE")
fi
aws_cli() {
  aws "${AWS_ARGS[@]}" "$@"
}

create_security_group() {
  local vpc_id=$1
  local name=$2
  local sg_id
  sg_id=$(aws_cli ec2 create-security-group \
    --group-name "$name" \
    --description "Beach Road auto SG" \
    --vpc-id "$vpc_id" \
    --query 'GroupId' \
    --output text)
  aws_cli ec2 authorize-security-group-ingress --group-id "$sg_id" --protocol tcp --port 22 --cidr 0.0.0.0/0 >/dev/null 2>&1 || true
  aws_cli ec2 authorize-security-group-ingress --group-id "$sg_id" --protocol tcp --port 80 --cidr 0.0.0.0/0 >/dev/null 2>&1 || true
  aws_cli ec2 authorize-security-group-ingress --group-id "$sg_id" --protocol tcp --port 443 --cidr 0.0.0.0/0 >/dev/null 2>&1 || true
  aws_cli ec2 authorize-security-group-ingress --group-id "$sg_id" --protocol tcp --port 8080 --cidr 0.0.0.0/0 >/dev/null 2>&1 || true
  echo "$sg_id"
}

create_instance() {
  if [[ -z $KEY_NAME ]]; then
    echo "--key-name is required when creating a new instance." >&2
    exit 1
  fi
  if [[ -z $SSH_KEY ]]; then
    echo "--ssh-key is required when creating a new instance." >&2
    exit 1
  fi
  if [[ ! -f $SSH_KEY ]]; then
    echo "SSH key not found: $SSH_KEY" >&2
    exit 1
  fi
  echo "Creating new EC2 instance in $AWS_REGION..."
  local vpc_id
  vpc_id=$(aws_cli ec2 describe-vpcs --filters Name=isDefault,Values=true --query 'Vpcs[0].VpcId' --output text)
  if [[ -z $vpc_id || $vpc_id == "None" ]]; then
    echo "Could not determine default VPC in region $AWS_REGION." >&2
    exit 1
  fi
  local subnet_id
  subnet_id=$(aws_cli ec2 describe-subnets \
    --filters Name=vpc-id,Values="$vpc_id" Name=default-for-az,Values=true \
    --query 'Subnets[0].SubnetId' --output text)
  if [[ -z $subnet_id || $subnet_id == "None" ]]; then
    subnet_id=$(aws_cli ec2 describe-subnets --filters Name=vpc-id,Values="$vpc_id" --query 'Subnets[0].SubnetId' --output text)
  fi
  if [[ -z $subnet_id || $subnet_id == "None" ]]; then
    echo "Failed to locate a subnet in VPC $vpc_id." >&2
    exit 1
  fi

  local sg_id=$SECURITY_GROUP_ID
  if [[ -z $sg_id ]]; then
    local sg_name="beach-road-$(date +%s)"
    sg_id=$(create_security_group "$vpc_id" "$sg_name")
    echo "Created security group $sg_id (open 22,8080)."
    SECURITY_GROUP_ID=$sg_id
  fi

  local ami
  ami=$(aws_cli ssm get-parameter \
    --name /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 \
    --query 'Parameter.Value' --output text)
  if [[ -z $ami || $ami == "None" ]]; then
    echo "Failed to resolve Amazon Linux 2023 AMI." >&2
    exit 1
  fi

  local user_data_file
  user_data_file=$(mktemp)
  cat <<'USERDATA' > "$user_data_file"
#!/bin/bash
set -euxo pipefail
dnf update -y
dnf install -y docker git nginx certbot python3-certbot-nginx
systemctl enable --now docker
if ! docker ps --format '{{.Names}}' | grep -q '^beach-redis$'; then
  docker run -d --name beach-redis --restart unless-stopped -p 6379:6379 redis:7-alpine || true
fi
useradd --system --home /opt/beach-road --shell /usr/sbin/nologin beach || true
install -d -o beach -g beach -m 750 /opt/beach-road || true
USERDATA

  local run_args=(
    ec2 run-instances
    --image-id "$ami"
    --instance-type "$INSTANCE_TYPE"
    --key-name "$KEY_NAME"
    --security-group-ids "$sg_id"
    --subnet-id "$subnet_id"
    --user-data "file://$user_data_file"
    --associate-public-ip-address
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=beach-road}]"
  )

  local instance_id
  instance_id=$(aws_cli "${run_args[@]}" --query 'Instances[0].InstanceId' --output text)
  rm -f "$user_data_file"
  if [[ -z $instance_id || $instance_id == "None" ]]; then
    echo "run-instances did not return an instance id." >&2
    exit 1
  fi
  echo "Launched instance $instance_id. Waiting for it to become ready..."
  aws_cli ec2 wait instance-running --instance-ids "$instance_id"
  aws_cli ec2 wait instance-status-ok --instance-ids "$instance_id"
  INSTANCE_ID=$instance_id
  local ip
  ip=$(aws_cli ec2 describe-instances --instance-ids "$instance_id" --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)
  if [[ -z $ip || $ip == "None" ]]; then
    HOST=""
  else
    HOST="$ip"
  fi
  echo "Instance ready: $instance_id (public IP: $ip)"
}

allocate_eip_if_needed() {
  if [[ -z $INSTANCE_ID ]]; then
    return
  fi
  echo "Ensuring instance $INSTANCE_ID has a public IP..."
  local alloc_json alloc_id ip
  alloc_json=$(aws_cli ec2 allocate-address --domain vpc)
  alloc_id=$(echo "$alloc_json" | python3 -c 'import sys,json; print(json.load(sys.stdin)["AllocationId"])')
  ip=$(echo "$alloc_json" | python3 -c 'import sys,json; print(json.load(sys.stdin)["PublicIp"])')
  if [[ -z $alloc_id || -z $ip ]]; then
    echo "Failed to allocate Elastic IP." >&2
    exit 1
  fi
  aws_cli ec2 associate-address --instance-id "$INSTANCE_ID" --allocation-id "$alloc_id" >/dev/null
  HOST="$ip"
  echo "Elastic IP $ip associated with instance $INSTANCE_ID."
}

if [[ -z $HOST && -z $INSTANCE_ID ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then
    echo "-- Dry run -- would create new instance in $AWS_REGION (type $INSTANCE_TYPE)."
  else
    create_instance
  fi
fi

if [[ -n $INSTANCE_ID ]]; then
  if [[ -z $HOST ]]; then
    if [[ $DRY_RUN -eq 1 ]]; then
      echo "-- Dry run -- would look up public IP for $INSTANCE_ID."
    else
      HOST=$(aws_cli ec2 describe-instances \
        --instance-ids "$INSTANCE_ID" \
        --query 'Reservations[0].Instances[0].PublicIpAddress' \
        --output text)
    fi
  fi
  if [[ -z $HOST || $HOST == "None" ]]; then
    if [[ $ENSURE_EIP -eq 0 ]]; then
      echo "Instance $INSTANCE_ID has no public IP. Use --allocate-eip or supply --host." >&2
      exit 1
    fi
    if [[ $DRY_RUN -eq 1 ]]; then
      echo "-- Dry run -- would allocate Elastic IP for $INSTANCE_ID."
      HOST="<pending-eip>"
    else
      allocate_eip_if_needed
    fi
  fi
fi

if [[ -n $HOST && $DRY_RUN -eq 1 ]]; then
  echo "Target host: $HOST"
fi

SSH_OPTS=("-o" "StrictHostKeyChecking=accept-new")
if [[ -n $SSH_KEY ]]; then
  if [[ ! -f $SSH_KEY ]]; then
    echo "SSH key not found: $SSH_KEY" >&2
    exit 1
  fi
  SSH_OPTS+=("-i" "$SSH_KEY")
else
  if [[ -n $INSTANCE_ID || -z $HOST ]]; then
    echo "--ssh-key is required for SSH operations." >&2
    exit 1
  fi
fi

BINARY_PATH="$PROJECT_ROOT/target/$TARGET/$PROFILE/beach-road"
if [[ $SKIP_BUILD -eq 0 ]]; then
  echo "Building beach-road (target $TARGET, profile $PROFILE)..."
  if [[ $DRY_RUN -eq 1 ]]; then
    :
  else
    pushd "$APP_DIR" >/dev/null
    cargo build --profile "$PROFILE" --target "$TARGET"
    popd >/dev/null
  fi
else
  echo "Skipping build step as requested."
fi

if [[ $DRY_RUN -eq 1 ]]; then
  echo "-- Dry run -- deployment steps would execute now."
  exit 0
fi

if [[ ! -f $BINARY_PATH ]]; then
  echo "Compiled binary not found at $BINARY_PATH" >&2
  exit 1
fi

if [[ -z $HOST ]]; then
  echo "Unable to determine target host." >&2
  exit 1
fi

echo "Deploying to $REMOTE_USER@$HOST (remote dir $REMOTE_DIR)"

ts=$(date +%s)
REMOTE_TMP_BIN="/tmp/beach-road-${ts}.bin"

scp "${SSH_OPTS[@]}" "$BINARY_PATH" "$REMOTE_USER@$HOST:$REMOTE_TMP_BIN"
if [[ -n $ENV_FILE ]]; then
  REMOTE_TMP_ENV="/tmp/beach-road-${ts}.env"
  scp "${SSH_OPTS[@]}" "$ENV_FILE" "$REMOTE_USER@$HOST:$REMOTE_TMP_ENV"
else
  REMOTE_TMP_ENV=""
fi

ssh "${SSH_OPTS[@]}" "$REMOTE_USER@$HOST" bash -s "$REMOTE_TMP_BIN" "${REMOTE_TMP_ENV}" "$REMOTE_DIR" <<'EOSSH'
set -euo pipefail
TMP_BIN="$1"
TMP_ENV="$2"
REMOTE_DIR="$3"

sudo id beach >/dev/null 2>&1 || sudo useradd --system --home /opt/beach-road --shell /usr/sbin/nologin beach
sudo install -d -o beach -g beach -m 750 "${REMOTE_DIR}"
sudo install -o beach -g beach -m 750 "${TMP_BIN}" "${REMOTE_DIR}/beach-road"
if [[ -n "${TMP_ENV}" && -f "${TMP_ENV}" ]]; then
  sudo install -o beach -g beach -m 640 "${TMP_ENV}" "${REMOTE_DIR}/beach-road.env"
fi
sudo tee /etc/systemd/system/beach-road.service >/dev/null <<'SERVICE'
[Unit]
Description=Beach Road Service
After=network.target docker.service
Requires=docker.service

[Service]
EnvironmentFile=-/opt/beach-road/beach-road.env
ExecStart=/opt/beach-road/beach-road
Restart=always
User=beach
Group=beach
WorkingDirectory=/opt/beach-road

[Install]
WantedBy=multi-user.target
SERVICE
sudo systemctl daemon-reload
sudo systemctl enable beach-road >/dev/null 2>&1 || true
sudo systemctl restart beach-road

sudo rm -f "${TMP_BIN}"
if [[ -n "${TMP_ENV}" ]]; then
  sudo rm -f "${TMP_ENV}"
fi

if command -v docker >/dev/null 2>&1; then
  if ! sudo docker ps --format '{{.Names}}' | grep -q '^beach-redis$'; then
    sudo docker run -d --name beach-redis --restart unless-stopped -p 6379:6379 redis:7-alpine >/dev/null
  fi
fi

# Setup nginx if not already configured
if [ ! -f /etc/nginx/conf.d/beach-road.conf ]; then
  sudo tee /etc/nginx/conf.d/beach-road.conf >/dev/null <<'NGINXCONF'
server {
    listen 80;
    server_name api.beach.sh;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 86400;
    }
}
NGINXCONF
  sudo systemctl enable --now nginx >/dev/null 2>&1 || true
fi

sudo systemctl status --no-pager beach-road
EOSSH

echo "Deployment complete."
if [[ -n $INSTANCE_ID ]]; then
  echo "Instance ID: $INSTANCE_ID"
fi
if [[ -n $SECURITY_GROUP_ID ]]; then
  echo "Security Group: $SECURITY_GROUP_ID"
fi
