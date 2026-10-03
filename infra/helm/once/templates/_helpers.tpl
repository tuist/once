{{- define "once.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "once.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name (include "once.name" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "once.labels" -}}
app.kubernetes.io/name: {{ include "once.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{- end -}}

{{- define "once.selectorLabels" -}}
app.kubernetes.io/name: {{ include "once.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "once.webLabels" -}}
{{ include "once.labels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "once.webSelectorLabels" -}}
{{ include "once.selectorLabels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "once.appSecretName" -}}
{{ include "once.fullname" . }}-app
{{- end -}}

{{- define "once.postgresClusterName" -}}
{{ include "once.fullname" . }}-postgres
{{- end -}}

{{- define "once.postgresAppSecret" -}}
{{ include "once.postgresClusterName" . }}-app
{{- end -}}
