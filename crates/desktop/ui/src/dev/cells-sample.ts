// Dev only: the code both cell fixtures show under `?hl` (highlighting check). Rendered, never run.
export const SAMPLE = `#| label: East pre-flight (read-only)
#| name: east_preflight
#| cwd: /tmp/cellhl/example-app-connector
#| expect: s3 ls exit 1
#| expect: running: 5
#| expect: sgr-0000000000000000a
#| expect: sgr-0000000000000000b
#| expect: sgr-0000000000000000c
aws s3 ls --profile example s3://example-state-bucket/.pulumi/stacks/example-app-connector/; echo "s3 ls exit $?"
aws ec2 describe-instances --profile example --region us-east-1 --instance-ids i-0000000000000000a i-0000000000000000b --query 'sort_by(Reservations[].Instances[], &Tags[?Key==\`Name\`]|[0].Value)[].[Tags[?Key==\`Name\`]|[0].Value,InstanceId,LaunchTime,State.Name]' --output text | tee /tmp/example-east-instances.before
echo "running: $(grep -c running /tmp/example-east-instances.before)"
aws ec2 describe-security-group-rules --profile example --region us-east-1 --filters Name=group-id,Values=sg-00000000000000000 --query 'sort_by(SecurityGroupRules, &SecurityGroupRuleId)[].[SecurityGroupRuleId,IsEgress,IpProtocol,CidrIpv4,Description]' --output text | tee /tmp/example-east-sg-rules.before`

export const PYTHON = `#| name: top-projects
#| label: Top projects
import pandas as pd  # the usage export
df = pd.read_csv("usage-2026-10.csv")
top = df.groupby("project").cost_usd.sum().nlargest(3)
print(f"{len(df)} rows, top: {top.index[0]!r}")`

export const PWSH = `#| name: disk-free
#| label: Disk free
# free space per drive, in GB
$drives = Get-PSDrive -PSProvider FileSystem
foreach ($d in $drives) { Write-Output ("{0}: {1:N1} GB" -f $d.Name, ($d.Free / 1GB)) }`

/** The sample's code without its header: a plain (non-cell) bash fence or code block. */
export const PLAIN = `# plain bash, no cell header
${SAMPLE.split('\n').filter((l) => !l.startsWith('#|')).join('\n')}`
