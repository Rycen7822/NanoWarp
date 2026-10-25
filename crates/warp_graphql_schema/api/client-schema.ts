import fetch from 'cross-fetch';
import { getIntrospectionQuery, buildClientSchema, GraphQLSchema } from 'graphql';
import { filterSchema, pruneSchema } from '@graphql-tools/utils';

const clientMutations = [
  'confirmFileArtifactUpload',
  'createAgentTask',
  'createAnonymousUser',
  'createFileArtifactUploadTarget',
  'createManagedMcpClientConfig',
  'createManagedSecret',
  'deleteConversation',
  'deleteManagedSecret',
  'updateManagedSecret',
  'expireApiKey',
  'generateApiKey',
  'generateCodeEmbeddings',
  'generateUniqueUpgradePromoCode',
  'issueTaskIdentityToken',
  'markAcceptedIntelligentAutosuggestion',
  'mintCustomToken',
  'populateMerkleTreeCache',
  'prewarmedSandboxConfig',
  'provideNegativeFeedbackResponseForAiConversation',
  'sendOnboardingSurveyResponses',
  'setUserIsOnboarded',
  'updateAgentTask',
  'updateMerkleTree',
  'updateOnboardingSurveyStatus',
  'createSimpleIntegration',
];

const clientQueries = [
  'codebaseContextConfig',
  'freeAvailableModels',
  'getRelevantFragments',
  'rerankFragments',
  'harnessAuthSecrets',
  'listWarpDevImages',
  'pricingInfo',
  'managedSecrets',
  'syncMerkleTree',
  'user',
  'userGithubInfo',
  'userRepoAuthStatus',
  'apiKeys',
  'getOAuthConnectTxStatus',
  'getCloudEnvironments',
  'simpleIntegrations',
  'getIntegrationsUsingEnvironment',
  'task',
  'taskSecrets',
  'tuiOnboardingMarkers',
  'listAIConversations',
  'suggestCloudEnvironmentImage'
];

function filterToClient(schema: GraphQLSchema): GraphQLSchema {
  const filtered = filterSchema({
    schema,
    rootFieldFilter: (operation, rootFieldName) => {
      if (operation === 'Query') {
        return clientQueries.includes(rootFieldName);
      } else if (operation === 'Mutation') {
        return clientMutations.includes(rootFieldName);
      } else {
        return false;
      }
    }
  });

  return pruneSchema(filtered);
}

module.exports = async (schemaUrl: string) => {
  // Adapted from https://the-guild.dev/graphql/codegen/docs/config-reference/schema-field#custom-schema-loader
  const response = await fetch(schemaUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ query: getIntrospectionQuery() }),
  });
  const data = await response.json();
  const schema = buildClientSchema(data.data);
  return filterToClient(schema);
};
