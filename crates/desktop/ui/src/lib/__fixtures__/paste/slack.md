Deploy is done :tada:, dashboard: [grafana](https://grafana.contoso.com/d/abc?orgId=1) cc [@sky](https://contoso.slack.com/team/U02ABC)
Rollback: `make rollback ENV=prod`

```
kubectl rollout status deploy/api
kubectl get pods -l app=api
```
