{{- define "struo.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "struo.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name (include "struo.name" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "struo.labels" -}}
app.kubernetes.io/name: {{ include "struo.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{- end -}}

{{- define "struo.selectorLabels" -}}
app.kubernetes.io/name: {{ include "struo.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "struo.webLabels" -}}
{{ include "struo.labels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "struo.webSelectorLabels" -}}
{{ include "struo.selectorLabels" . }}
app.kubernetes.io/component: web
{{- end -}}

{{- define "struo.appSecretName" -}}
{{ include "struo.fullname" . }}-app
{{- end -}}
